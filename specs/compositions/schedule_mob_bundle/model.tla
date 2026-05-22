---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for schedule_mob_bundle.

CONSTANTS ClaimTokenValues, DeliveryReceiptStageValues, MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, OccurrenceTransitionFailureObservationKindValues, OverlapPolicyValues, RuntimeCompletionOutcomeValues, ScheduleIdValues, SessionIdValues, SetOfOccurrenceIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionClaimTokenValues == {None} \cup {Some(x) : x \in ClaimTokenValues}
OptionDeliveryReceiptStageValues == {None} \cup {Some(x) : x \in DeliveryReceiptStageValues}
OptionOccurrenceFailureClassValues == {None} \cup {Some(x) : x \in OccurrenceFailureClassValues}
OptionSessionIdValues == {None} \cup {Some(x) : x \in SessionIdValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
MapIncrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) + amount ELSE map[x]]
MapDecrement(map, key, amount) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN (IF key \in DOMAIN map THEN map[key] ELSE 0) - amount ELSE map[x]]
MapRemove(map, key) == [x \in DOMAIN map \ {key} |-> map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
Count(seq, value) == Cardinality({i \in DOMAIN seq : seq[i] = value})
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN Tail(seq) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))
AppendIfMissing(seq, value) == IF value \in SeqElements(seq) THEN seq ELSE Append(seq, value)
Machines == {
    <<"occurrence", "OccurrenceLifecycleMachine", "occurrence_authority">>,
    <<"schedule", "ScheduleLifecycleMachine", "schedule_authority">>
}

RouteNames == {
    "revision_supersede_enters_occurrence_authority",
    "occurrence_supersede_ack_returns_to_schedule"
}

Actors == {
    "occurrence_authority",
    "schedule_authority"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "occurrence" -> "occurrence_authority"
      [] machine_id = "schedule" -> "schedule_authority"
      [] OTHER -> "unknown_actor"

RouteSource(route_name) ==
    CASE route_name = "revision_supersede_enters_occurrence_authority" -> "schedule"
      [] route_name = "occurrence_supersede_ack_returns_to_schedule" -> "occurrence"
      [] OTHER -> "unknown_machine"

RouteEffect(route_name) ==
    CASE route_name = "revision_supersede_enters_occurrence_authority" -> "SupersedePendingOccurrences"
      [] route_name = "occurrence_supersede_ack_returns_to_schedule" -> "OccurrencesSuperseded"
      [] OTHER -> "unknown_effect"

RouteTargetMachine(route_name) ==
    CASE route_name = "revision_supersede_enters_occurrence_authority" -> "occurrence"
      [] route_name = "occurrence_supersede_ack_returns_to_schedule" -> "schedule"
      [] OTHER -> "unknown_machine"

RouteTargetInput(route_name) ==
    CASE route_name = "revision_supersede_enters_occurrence_authority" -> "Supersede"
      [] route_name = "occurrence_supersede_ack_returns_to_schedule" -> "ConfirmOccurrencesSuperseded"
      [] OTHER -> "unknown_input"

RouteTargetKind(route_name) ==
    CASE route_name = "revision_supersede_enters_occurrence_authority" -> "Input"
      [] route_name = "occurrence_supersede_ack_returns_to_schedule" -> "Input"
      [] OTHER -> "Unknown"

RouteDeliveryKind(route_name) ==
    CASE route_name = "revision_supersede_enters_occurrence_authority" -> "Immediate"
      [] route_name = "occurrence_supersede_ack_returns_to_schedule" -> "Immediate"
      [] OTHER -> "Unknown"

RouteTargetActor(route_name) == ActorOfMachine(RouteTargetMachine(route_name))

VARIABLES occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ occurrence_phase = "Pending"
    /\ occurrence_occurrence_id = "occurrence-0"
    /\ occurrence_schedule_id = "schedule-0"
    /\ occurrence_schedule_revision = 1
    /\ occurrence_occurrence_ordinal = 0
    /\ occurrence_trigger_key = "trigger-0"
    /\ occurrence_target_binding_key = "target-0"
    /\ occurrence_misfire_policy = "Skip"
    /\ occurrence_misfire_policy_key = "misfire:skip"
    /\ occurrence_overlap_policy = "SkipIfRunning"
    /\ occurrence_overlap_policy_key = "overlap:skip_if_running"
    /\ occurrence_missing_target_policy = "MarkMisfired"
    /\ occurrence_missing_target_policy_key = "missing_target:mark_misfired"
    /\ occurrence_due_at_utc_ms = 1
    /\ occurrence_misfire_deadline_utc_ms = 1
    /\ occurrence_claimed_by = None
    /\ occurrence_lease_expires_at_utc_ms = None
    /\ occurrence_claimed_at_utc_ms = None
    /\ occurrence_claim_token = None
    /\ occurrence_delivery_correlation_id = None
    /\ occurrence_target_materialized_session_id = None
    /\ occurrence_receipt_recorded_at_utc_ms = None
    /\ occurrence_last_receipt_recorded_at_utc_ms = None
    /\ occurrence_last_receipt_attempt = None
    /\ occurrence_last_receipt_stage = None
    /\ occurrence_last_receipt_failure_class = None
    /\ occurrence_last_receipt_detail = None
    /\ occurrence_last_receipt_correlation_id = None
    /\ occurrence_last_receipt_materialized_session_id = None
    /\ occurrence_runtime_outcome_key = None
    /\ occurrence_receipt_stage = None
    /\ occurrence_receipt_failure_class = None
    /\ occurrence_receipt_detail = None
    /\ occurrence_failure_class = None
    /\ occurrence_failure_detail = None
    /\ occurrence_dispatched_at_utc_ms = None
    /\ occurrence_completed_at_utc_ms = None
    /\ occurrence_attempt_count = 0
    /\ occurrence_superseded_by_revision = None
    /\ schedule_phase = "Active"
    /\ schedule_schedule_id = "schedule-0"
    /\ schedule_revision = 1
    /\ schedule_trigger_key = "trigger-0"
    /\ schedule_target_binding_key = "target-0"
    /\ schedule_misfire_policy = "Skip"
    /\ schedule_misfire_policy_key = "misfire:skip"
    /\ schedule_overlap_policy = "SkipIfRunning"
    /\ schedule_overlap_policy_key = "overlap:skip_if_running"
    /\ schedule_missing_target_policy = "MarkMisfired"
    /\ schedule_missing_target_policy_key = "missing_target:mark_misfired"
    /\ schedule_planning_horizon_days = 30
    /\ schedule_planning_horizon_occurrences = 64
    /\ schedule_planning_cursor_utc_ms = None
    /\ schedule_next_occurrence_ordinal = 0
    /\ schedule_superseded_ack_ids = {}
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

WitnessInit_revision_supersede_route ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

WitnessInit_occurrence_supersede_ack_route ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

occurrence__is_live_claim_phase(arg_phase) == ((arg_phase = "Claimed") \/ (arg_phase = "Dispatching") \/ (arg_phase = "AwaitingCompletion"))

occurrence_ClassifyTransitionFailurePlanRejectedPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailurePlanRejectedDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ (packet.payload.observation = "PlanOccurrence")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "PlanRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailurePlanRejectedDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailurePlanRejectedDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ (packet.payload.observation = "SyncTargetSnapshot")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "TargetSyncRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ (packet.payload.observation = "RecordReceipt")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "ReceiptRecordRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ (packet.payload.observation = "ClassifyDue")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "DueClassificationRejected"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ (packet.payload.observation = "Claim")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotPendingForClaim"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotPendingForClaimDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotPendingForClaimDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotClaimedDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ (packet.payload.observation = "DispatchStarted")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotClaimed"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotClaimedDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotClaimedDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotDispatchingDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ (packet.payload.observation = "AwaitCompletion")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotDispatching"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotDispatchingDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotDispatchingDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ ((packet.payload.observation = "LeaseExpired") \/ (packet.payload.observation = "ReleaseLeaseForPausedSchedule"))
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLeaseHolding"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalPending(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalPending"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalClaimed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalDispatching(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalCompleted(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalSkipped(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalSkipped"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalMisfired(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalMisfired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalSuperseded(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalSuperseded"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(arg_observation) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyTransitionFailure"
       /\ packet.payload.observation = arg_observation
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ ((packet.payload.observation = "Complete") \/ (packet.payload.observation = "ResolveRuntimeCompletion") \/ (packet.payload.observation = "Skip") \/ (packet.payload.observation = "Misfire") \/ (packet.payload.observation = "Supersede") \/ (packet.payload.observation = "DeliveryFailed"))
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "TransitionFailureClassified", payload |-> [observation |-> packet.payload.observation, public_class |-> "NotLiveForTerminal"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "PlanOccurrence"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.schedule_id = arg_schedule_id
       /\ packet.payload.schedule_revision = arg_schedule_revision
       /\ packet.payload.occurrence_ordinal = arg_occurrence_ordinal
       /\ packet.payload.trigger_key = arg_trigger_key
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.misfire_policy = arg_misfire_policy
       /\ packet.payload.misfire_policy_key = arg_misfire_policy_key
       /\ packet.payload.overlap_policy = arg_overlap_policy
       /\ packet.payload.overlap_policy_key = arg_overlap_policy_key
       /\ packet.payload.missing_target_policy = arg_missing_target_policy
       /\ packet.payload.missing_target_policy_key = arg_missing_target_policy_key
       /\ packet.payload.target_materialized_session_id = arg_target_materialized_session_id
       /\ packet.payload.due_at_utc_ms = arg_due_at_utc_ms
       /\ packet.payload.misfire_deadline_utc_ms = arg_misfire_deadline_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ ((occurrence_attempt_count = 0) /\ (occurrence_claimed_by = None) /\ (occurrence_claim_token = None) /\ (occurrence_delivery_correlation_id = None) /\ (occurrence_target_materialized_session_id = None) /\ (occurrence_completed_at_utc_ms = None) /\ (occurrence_superseded_by_revision = None) /\ (packet.payload.misfire_deadline_utc_ms >= packet.payload.due_at_utc_ms))
       /\ occurrence_phase' = "Pending"
       /\ occurrence_occurrence_id' = packet.payload.occurrence_id
       /\ occurrence_schedule_id' = packet.payload.schedule_id
       /\ occurrence_schedule_revision' = packet.payload.schedule_revision
       /\ occurrence_occurrence_ordinal' = packet.payload.occurrence_ordinal
       /\ occurrence_trigger_key' = packet.payload.trigger_key
       /\ occurrence_target_binding_key' = packet.payload.target_binding_key
       /\ occurrence_misfire_policy' = packet.payload.misfire_policy
       /\ occurrence_misfire_policy_key' = packet.payload.misfire_policy_key
       /\ occurrence_overlap_policy' = packet.payload.overlap_policy
       /\ occurrence_overlap_policy_key' = packet.payload.overlap_policy_key
       /\ occurrence_missing_target_policy' = packet.payload.missing_target_policy
       /\ occurrence_missing_target_policy_key' = packet.payload.missing_target_policy_key
       /\ occurrence_due_at_utc_ms' = packet.payload.due_at_utc_ms
       /\ occurrence_misfire_deadline_utc_ms' = packet.payload.misfire_deadline_utc_ms
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claimed_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_target_materialized_session_id' = packet.payload.target_materialized_session_id
       /\ occurrence_receipt_recorded_at_utc_ms' = None
       /\ occurrence_last_receipt_recorded_at_utc_ms' = None
       /\ occurrence_last_receipt_attempt' = None
       /\ occurrence_last_receipt_stage' = None
       /\ occurrence_last_receipt_failure_class' = None
       /\ occurrence_last_receipt_detail' = None
       /\ occurrence_last_receipt_correlation_id' = None
       /\ occurrence_last_receipt_materialized_session_id' = None
       /\ occurrence_runtime_outcome_key' = None
       /\ occurrence_receipt_stage' = None
       /\ occurrence_receipt_failure_class' = None
       /\ occurrence_receipt_detail' = None
       /\ occurrence_failure_class' = None
       /\ occurrence_failure_detail' = None
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ occurrence_completed_at_utc_ms' = None
       /\ occurrence_attempt_count' = 0
       /\ occurrence_superseded_by_revision' = None
       /\ UNCHANGED << schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "PlanOccurrenceFromPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDuePendingFuture(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ (packet.payload.now_utc_ms < occurrence_due_at_utc_ms)
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDuePendingFuture"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDuePendingFuture", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDuePendingMisfire(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ ((occurrence_due_at_utc_ms <= packet.payload.now_utc_ms) /\ (occurrence_misfire_deadline_utc_ms < packet.payload.now_utc_ms))
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueMisfireRequired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDuePendingMisfire"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDuePendingMisfire", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDuePendingClaimEligible(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ ((occurrence_due_at_utc_ms <= packet.payload.now_utc_ms) /\ (packet.payload.now_utc_ms <= occurrence_misfire_deadline_utc_ms))
       /\ occurrence_phase' = "Pending"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueClaimEligible", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDuePendingClaimEligible"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDuePendingClaimEligible", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueClaimedLeaseExpired(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ ((occurrence_lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN occurrence_lease_expires_at_utc_ms THEN occurrence_lease_expires_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueLeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueClaimedLeaseExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueClaimedLeaseExpired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueDispatchingLeaseExpired(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ ((occurrence_lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN occurrence_lease_expires_at_utc_ms THEN occurrence_lease_expires_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueLeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueDispatchingLeaseExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueDispatchingLeaseExpired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueAwaitingCompletionLeaseExpired(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ ((occurrence_lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN occurrence_lease_expires_at_utc_ms THEN occurrence_lease_expires_at_utc_ms["value"] ELSE None) <= packet.payload.now_utc_ms))
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueLeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueAwaitingCompletionLeaseExpired"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueAwaitingCompletionLeaseExpired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueClaimedLeaseCurrent(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ ((occurrence_lease_expires_at_utc_ms = None) \/ (packet.payload.now_utc_ms < (IF "value" \in DOMAIN occurrence_lease_expires_at_utc_ms THEN occurrence_lease_expires_at_utc_ms["value"] ELSE None)))
       /\ occurrence_phase' = "Claimed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueClaimedLeaseCurrent"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueClaimedLeaseCurrent", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueDispatchingLeaseCurrent(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ ((occurrence_lease_expires_at_utc_ms = None) \/ (packet.payload.now_utc_ms < (IF "value" \in DOMAIN occurrence_lease_expires_at_utc_ms THEN occurrence_lease_expires_at_utc_ms["value"] ELSE None)))
       /\ occurrence_phase' = "Dispatching"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueDispatchingLeaseCurrent"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueDispatchingLeaseCurrent", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueAwaitingCompletionLeaseCurrent(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ ((occurrence_lease_expires_at_utc_ms = None) \/ (packet.payload.now_utc_ms < (IF "value" \in DOMAIN occurrence_lease_expires_at_utc_ms THEN occurrence_lease_expires_at_utc_ms["value"] ELSE None)))
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueAwaitingCompletionLeaseCurrent"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueAwaitingCompletionLeaseCurrent", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueCompletedNoAction(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ occurrence_phase' = "Completed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueCompletedNoAction"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueCompletedNoAction", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueSkippedNoAction(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ occurrence_phase' = "Skipped"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueSkippedNoAction"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueSkippedNoAction", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueMisfiredNoAction(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ occurrence_phase' = "Misfired"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueMisfiredNoAction"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueMisfiredNoAction", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueSupersededNoAction(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ occurrence_phase' = "Superseded"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueSupersededNoAction"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueSupersededNoAction", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ClassifyDueDeliveryFailedNoAction(arg_now_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ClassifyDue"
       /\ packet.payload.now_utc_ms = arg_now_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ occurrence_phase' = "DeliveryFailed"
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DueNoAction", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ClassifyDueDeliveryFailedNoAction"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ClassifyDueDeliveryFailedNoAction", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "SyncTargetSnapshot"
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.target_materialized_session_id = arg_target_materialized_session_id
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ occurrence_phase' = "Pending"
       /\ occurrence_target_binding_key' = packet.payload.target_binding_key
       /\ occurrence_target_materialized_session_id' = packet.payload.target_materialized_session_id
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "SyncTargetSnapshotPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "SyncTargetSnapshot"
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.target_materialized_session_id = arg_target_materialized_session_id
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ occurrence_phase' = "Claimed"
       /\ occurrence_target_binding_key' = packet.payload.target_binding_key
       /\ occurrence_target_materialized_session_id' = packet.payload.target_materialized_session_id
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "SyncTargetSnapshotClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptPending(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Pending"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "Pending"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptPending", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptClaimed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "Claimed"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Claimed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptDispatching(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "Dispatching"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Dispatching"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptAwaitingCompletion(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "AwaitingCompletion"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptCompleted(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Completed"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "Completed"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptSkipped(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Skipped"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "Skipped"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptSkipped", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Skipped"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptMisfired(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Misfired"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "Misfired"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptMisfired", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Misfired"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptSuperseded(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Superseded"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "Superseded"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptSuperseded", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RecordReceiptDeliveryFailed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "RecordReceipt"
       /\ packet.payload.correlation_id = arg_correlation_id
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.materialized_session_id = arg_materialized_session_id
       /\ packet.payload.runtime_outcome_key = arg_runtime_outcome_key
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "DeliveryFailed"
       /\ ((occurrence_receipt_stage # None) /\ (occurrence_receipt_recorded_at_utc_ms # None) /\ (occurrence_receipt_detail = packet.payload.detail) /\ (occurrence_delivery_correlation_id = packet.payload.correlation_id) /\ (occurrence_target_materialized_session_id = packet.payload.materialized_session_id))
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_last_receipt_recorded_at_utc_ms' = occurrence_receipt_recorded_at_utc_ms
       /\ occurrence_last_receipt_attempt' = Some(occurrence_attempt_count)
       /\ occurrence_last_receipt_stage' = occurrence_receipt_stage
       /\ occurrence_last_receipt_failure_class' = occurrence_receipt_failure_class
       /\ occurrence_last_receipt_detail' = packet.payload.detail
       /\ occurrence_last_receipt_correlation_id' = packet.payload.correlation_id
       /\ occurrence_last_receipt_materialized_session_id' = packet.payload.materialized_session_id
       /\ occurrence_runtime_outcome_key' = packet.payload.runtime_outcome_key
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RecordReceiptDeliveryFailed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


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
       /\ ((occurrence_due_at_utc_ms <= packet.payload.at_utc_ms) /\ (packet.payload.at_utc_ms <= occurrence_misfire_deadline_utc_ms))
       /\ occurrence_phase' = "Claimed"
       /\ occurrence_claimed_by' = Some(packet.payload.owner_id)
       /\ occurrence_lease_expires_at_utc_ms' = Some(packet.payload.lease_expires_at_utc_ms)
       /\ occurrence_claimed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_claim_token' = Some(packet.payload.claim_token)
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_receipt_recorded_at_utc_ms' = None
       /\ occurrence_last_receipt_recorded_at_utc_ms' = None
       /\ occurrence_last_receipt_attempt' = None
       /\ occurrence_last_receipt_stage' = None
       /\ occurrence_last_receipt_failure_class' = None
       /\ occurrence_last_receipt_detail' = None
       /\ occurrence_last_receipt_correlation_id' = None
       /\ occurrence_last_receipt_materialized_session_id' = None
       /\ occurrence_runtime_outcome_key' = None
       /\ occurrence_receipt_stage' = None
       /\ occurrence_receipt_failure_class' = None
       /\ occurrence_receipt_detail' = None
       /\ occurrence_failure_class' = None
       /\ occurrence_failure_detail' = None
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ occurrence_completed_at_utc_ms' = None
       /\ occurrence_attempt_count' = (occurrence_attempt_count) + 1
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_target_materialized_session_id, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("DispatchStarted")
       /\ occurrence_receipt_failure_class' = None
       /\ occurrence_receipt_detail' = None
       /\ occurrence_dispatched_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AwaitingCompletion", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AwaitCompletionFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "AwaitCompletionFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_CompleteFromDispatchingOrAwaiting(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Complete"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Completed"
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("Completed")
       /\ occurrence_receipt_failure_class' = None
       /\ occurrence_receipt_detail' = None
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Completed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "CompleteFromDispatchingOrAwaiting"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "CompleteFromDispatchingOrAwaiting", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeCompletionCompleted(arg_outcome, arg_detail, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ResolveRuntimeCompletion"
       /\ packet.payload.outcome = arg_outcome
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.outcome = "Completed")
       /\ occurrence_phase' = "Completed"
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("Completed")
       /\ occurrence_receipt_failure_class' = None
       /\ occurrence_receipt_detail' = None
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Completed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeCompletionCompleted"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeCompletionCompleted", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Completed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeCompletionRuntimeRejected(arg_outcome, arg_detail, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ResolveRuntimeCompletion"
       /\ packet.payload.outcome = arg_outcome
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ ((packet.payload.outcome = "CallbackPending") \/ (packet.payload.outcome = "Cancelled") \/ (packet.payload.outcome = "Abandoned"))
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("DeliveryFailed")
       /\ occurrence_receipt_failure_class' = Some("RuntimeRejected")
       /\ occurrence_receipt_detail' = packet.payload.detail
       /\ occurrence_failure_class' = Some("RuntimeRejected")
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DeliveryFailed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeCompletionRuntimeRejected"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeCompletionRuntimeRejected", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeCompletionTransportError(arg_outcome, arg_detail, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ResolveRuntimeCompletion"
       /\ packet.payload.outcome = arg_outcome
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.outcome = "RuntimeTerminated")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("DeliveryFailed")
       /\ occurrence_receipt_failure_class' = Some("TransportError")
       /\ occurrence_receipt_detail' = packet.payload.detail
       /\ occurrence_failure_class' = Some("TransportError")
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DeliveryFailed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeCompletionTransportError"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeCompletionTransportError", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
       /\ model_step_count' = model_step_count + 1


occurrence_RuntimeCompletionInternalError(arg_outcome, arg_detail, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ResolveRuntimeCompletion"
       /\ packet.payload.outcome = arg_outcome
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ (packet.payload.outcome = "FinalizationFailed")
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("DeliveryFailed")
       /\ occurrence_receipt_failure_class' = Some("InternalError")
       /\ occurrence_receipt_detail' = packet.payload.detail
       /\ occurrence_failure_class' = Some("InternalError")
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "DeliveryFailed", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "RuntimeCompletionInternalError"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "RuntimeCompletionInternalError", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "DeliveryFailed"]}
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
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("Skipped")
       /\ occurrence_receipt_failure_class' = packet.payload.failure_class
       /\ occurrence_receipt_detail' = packet.payload.detail
       /\ occurrence_failure_class' = packet.payload.failure_class
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_at_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("Misfired")
       /\ occurrence_receipt_failure_class' = packet.payload.failure_class
       /\ occurrence_receipt_detail' = packet.payload.detail
       /\ occurrence_failure_class' = packet.payload.failure_class
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_at_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("Superseded")
       /\ occurrence_receipt_failure_class' = None
       /\ occurrence_receipt_detail' = None
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_superseded_by_revision' = Some(packet.payload.superseded_by_revision)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "schedule", variant |-> "ConfirmOccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], source_kind |-> "route", source_route |-> "occurrence_supersede_ack_returns_to_schedule", source_machine |-> "occurrence", source_effect |-> "OccurrencesSuperseded", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "schedule", variant |-> "ConfirmOccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], source_kind |-> "route", source_route |-> "occurrence_supersede_ack_returns_to_schedule", source_machine |-> "occurrence", source_effect |-> "OccurrencesSuperseded", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "occurrence_supersede_ack_returns_to_schedule", source_machine |-> "occurrence", effect |-> "OccurrencesSuperseded", target_machine |-> "schedule", target_input |-> "ConfirmOccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], actor |-> "schedule_authority", effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePendingOrLive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Superseded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePendingOrLive"], [machine |-> "occurrence", variant |-> "OccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePendingOrLive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "SupersedePendingOrLive", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_DeliveryFailedFromClaimedOrLive(arg_failure_class, arg_detail, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "DeliveryFailed"
       /\ packet.payload.failure_class = arg_failure_class
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed" \/ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("DeliveryFailed")
       /\ occurrence_receipt_failure_class' = Some(packet.payload.failure_class)
       /\ occurrence_receipt_detail' = packet.payload.detail
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_receipt_failure_class' = Some("LeaseLost")
       /\ occurrence_receipt_detail' = Some("lease expired before completion")
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_receipt_failure_class' = Some("LeaseLost")
       /\ occurrence_receipt_detail' = Some("lease expired before completion")
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_receipt_failure_class' = Some("LeaseLost")
       /\ occurrence_receipt_detail' = Some("lease expired before completion")
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "LeaseExpiredFromAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "LeaseExpiredFromAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ReleaseLeaseForPausedScheduleFromClaimed(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ReleaseLeaseForPausedSchedule"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed"
       /\ occurrence_phase' = "Pending"
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claimed_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_receipt_failure_class' = Some("LeaseLost")
       /\ occurrence_receipt_detail' = Some("lease released because schedule was paused before dispatch")
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseLeaseForPausedScheduleFromClaimed"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ReleaseLeaseForPausedScheduleFromClaimed", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ReleaseLeaseForPausedScheduleFromDispatching(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ReleaseLeaseForPausedSchedule"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching"
       /\ occurrence_phase' = "Pending"
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claimed_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_receipt_failure_class' = Some("LeaseLost")
       /\ occurrence_receipt_detail' = Some("lease released because schedule was paused before dispatch")
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseLeaseForPausedScheduleFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ReleaseLeaseForPausedScheduleFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_ReleaseLeaseForPausedScheduleFromAwaitingCompletion(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "ReleaseLeaseForPausedSchedule"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Pending"
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claimed_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_receipt_recorded_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ occurrence_receipt_stage' = Some("LeaseExpired")
       /\ occurrence_receipt_failure_class' = Some("LeaseLost")
       /\ occurrence_receipt_detail' = Some("lease released because schedule was paused before dispatch")
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ UNCHANGED << occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_target_materialized_session_id, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "LeaseExpired", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "ReleaseLeaseForPausedScheduleFromAwaitingCompletion"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "ReleaseLeaseForPausedScheduleFromAwaitingCompletion", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Pending"]}
       /\ model_step_count' = model_step_count + 1


occurrence_live_claim_requires_owner == (~(occurrence__is_live_claim_phase(occurrence_phase)) \/ (occurrence_claimed_by # None))
occurrence_superseded_records_revision == ((occurrence_phase # "Superseded") \/ (occurrence_superseded_by_revision # None))
occurrence_delivery_failed_records_failure_class == ((occurrence_phase # "DeliveryFailed") \/ (occurrence_failure_class # None))
occurrence_misfire_deadline_not_before_due == (occurrence_misfire_deadline_utc_ms >= occurrence_due_at_utc_ms)

schedule_CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Create"
       /\ packet.payload.schedule_id = arg_schedule_id
       /\ packet.payload.trigger_key = arg_trigger_key
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.misfire_policy = arg_misfire_policy
       /\ packet.payload.misfire_policy_key = arg_misfire_policy_key
       /\ packet.payload.overlap_policy = arg_overlap_policy
       /\ packet.payload.overlap_policy_key = arg_overlap_policy_key
       /\ packet.payload.missing_target_policy = arg_missing_target_policy
       /\ packet.payload.missing_target_policy_key = arg_missing_target_policy_key
       /\ packet.payload.planning_horizon_days = arg_planning_horizon_days
       /\ packet.payload.planning_horizon_occurrences = arg_planning_horizon_occurrences
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Active"
       /\ schedule_schedule_id' = packet.payload.schedule_id
       /\ schedule_trigger_key' = packet.payload.trigger_key
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ schedule_misfire_policy' = packet.payload.misfire_policy
       /\ schedule_misfire_policy_key' = packet.payload.misfire_policy_key
       /\ schedule_overlap_policy' = packet.payload.overlap_policy
       /\ schedule_overlap_policy_key' = packet.payload.overlap_policy_key
       /\ schedule_missing_target_policy' = packet.payload.missing_target_policy
       /\ schedule_missing_target_policy_key' = packet.payload.missing_target_policy_key
       /\ schedule_planning_horizon_days' = IF (packet.payload.planning_horizon_days # None) THEN (IF "value" \in DOMAIN packet.payload.planning_horizon_days THEN packet.payload.planning_horizon_days["value"] ELSE None) ELSE schedule_planning_horizon_days
       /\ schedule_planning_horizon_occurrences' = IF (packet.payload.planning_horizon_occurrences # None) THEN (IF "value" \in DOMAIN packet.payload.planning_horizon_occurrences THEN packet.payload.planning_horizon_occurrences["value"] ELSE None) ELSE schedule_planning_horizon_occurrences
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_revision, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Active", revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "CreateSchedule"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "CreateSchedule", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Revise"
       /\ packet.payload.trigger_key = arg_trigger_key
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.misfire_policy = arg_misfire_policy
       /\ packet.payload.misfire_policy_key = arg_misfire_policy_key
       /\ packet.payload.overlap_policy = arg_overlap_policy
       /\ packet.payload.overlap_policy_key = arg_overlap_policy_key
       /\ packet.payload.missing_target_policy = arg_missing_target_policy
       /\ packet.payload.missing_target_policy_key = arg_missing_target_policy_key
       /\ packet.payload.planning_horizon_days = arg_planning_horizon_days
       /\ packet.payload.planning_horizon_occurrences = arg_planning_horizon_occurrences
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Active"
       /\ schedule_revision' = (schedule_revision) + 1
       /\ schedule_trigger_key' = packet.payload.trigger_key
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ schedule_misfire_policy' = packet.payload.misfire_policy
       /\ schedule_misfire_policy_key' = packet.payload.misfire_policy_key
       /\ schedule_overlap_policy' = packet.payload.overlap_policy
       /\ schedule_overlap_policy_key' = packet.payload.overlap_policy_key
       /\ schedule_missing_target_policy' = packet.payload.missing_target_policy
       /\ schedule_missing_target_policy_key' = packet.payload.missing_target_policy_key
       /\ schedule_planning_horizon_days' = packet.payload.planning_horizon_days
       /\ schedule_planning_horizon_occurrences' = packet.payload.planning_horizon_occurrences
       /\ schedule_planning_cursor_utc_ms' = None
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "ReviseActive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Active", revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "ReviseActive"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "ReviseActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "ReviseActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Revise"
       /\ packet.payload.trigger_key = arg_trigger_key
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.misfire_policy = arg_misfire_policy
       /\ packet.payload.misfire_policy_key = arg_misfire_policy_key
       /\ packet.payload.overlap_policy = arg_overlap_policy
       /\ packet.payload.overlap_policy_key = arg_overlap_policy_key
       /\ packet.payload.missing_target_policy = arg_missing_target_policy
       /\ packet.payload.missing_target_policy_key = arg_missing_target_policy_key
       /\ packet.payload.planning_horizon_days = arg_planning_horizon_days
       /\ packet.payload.planning_horizon_occurrences = arg_planning_horizon_occurrences
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Paused"
       /\ schedule_phase' = "Paused"
       /\ schedule_revision' = (schedule_revision) + 1
       /\ schedule_trigger_key' = packet.payload.trigger_key
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ schedule_misfire_policy' = packet.payload.misfire_policy
       /\ schedule_misfire_policy_key' = packet.payload.misfire_policy_key
       /\ schedule_overlap_policy' = packet.payload.overlap_policy
       /\ schedule_overlap_policy_key' = packet.payload.overlap_policy_key
       /\ schedule_missing_target_policy' = packet.payload.missing_target_policy
       /\ schedule_missing_target_policy_key' = packet.payload.missing_target_policy_key
       /\ schedule_planning_horizon_days' = packet.payload.planning_horizon_days
       /\ schedule_planning_horizon_occurrences' = packet.payload.planning_horizon_occurrences
       /\ schedule_planning_cursor_utc_ms' = None
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "RevisePaused"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Paused", revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "RevisePaused"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "RevisePaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "RevisePaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Paused"]}
       /\ model_step_count' = model_step_count + 1


schedule_UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "UpdatePlanningConfig"
       /\ packet.payload.planning_horizon_days = arg_planning_horizon_days
       /\ packet.payload.planning_horizon_occurrences = arg_planning_horizon_occurrences
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Active"
       /\ schedule_planning_horizon_days' = packet.payload.planning_horizon_days
       /\ schedule_planning_horizon_occurrences' = packet.payload.planning_horizon_occurrences
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Active", revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "UpdatePlanningConfigActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "UpdatePlanningConfigActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "UpdatePlanningConfig"
       /\ packet.payload.planning_horizon_days = arg_planning_horizon_days
       /\ packet.payload.planning_horizon_occurrences = arg_planning_horizon_occurrences
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Paused"
       /\ schedule_phase' = "Paused"
       /\ schedule_planning_horizon_days' = packet.payload.planning_horizon_days
       /\ schedule_planning_horizon_occurrences' = packet.payload.planning_horizon_occurrences
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Paused", revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "UpdatePlanningConfigPaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "UpdatePlanningConfigPaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Paused"]}
       /\ model_step_count' = model_step_count + 1


schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "RecordPlanningWindow"
       /\ packet.payload.planning_cursor_utc_ms = arg_planning_cursor_utc_ms
       /\ packet.payload.next_occurrence_ordinal = arg_next_occurrence_ordinal
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ (packet.payload.next_occurrence_ordinal > 0)
       /\ schedule_phase' = "Active"
       /\ schedule_planning_cursor_utc_ms' = Some(packet.payload.planning_cursor_utc_ms)
       /\ schedule_next_occurrence_ordinal' = packet.payload.next_occurrence_ordinal
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Active", revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "RecordPlanningWindowActive"], [machine |-> "schedule", variant |-> "PlanningWindowRecorded", payload |-> [next_occurrence_ordinal |-> packet.payload.next_occurrence_ordinal, planning_cursor_utc_ms |-> packet.payload.planning_cursor_utc_ms], effect_id |-> (model_step_count + 1), source_transition |-> "RecordPlanningWindowActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "RecordPlanningWindowActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_SyncTargetSnapshotActive(arg_target_binding_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "SyncTargetSnapshot"
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Active"
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "SyncTargetSnapshotActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_SyncTargetSnapshotPaused(arg_target_binding_key) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "SyncTargetSnapshot"
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Paused"
       /\ schedule_phase' = "Paused"
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "SyncTargetSnapshotPaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Paused"]}
       /\ model_step_count' = model_step_count + 1


schedule_PauseActiveOrPaused(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Pause"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active" \/ schedule_phase = "Paused"
       /\ schedule_phase' = "Paused"
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Paused", revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "PauseActiveOrPaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "PauseActiveOrPaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Paused"]}
       /\ model_step_count' = model_step_count + 1


schedule_ResumeActiveOrPaused(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Resume"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active" \/ schedule_phase = "Paused"
       /\ schedule_phase' = "Active"
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Active", revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeActiveOrPaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "ResumeActiveOrPaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_DeleteActive(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Delete"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Deleted"
       /\ schedule_revision' = (schedule_revision) + 1
       /\ schedule_planning_cursor_utc_ms' = None
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "DeleteActive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Deleted", revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeleteActive"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeleteActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "DeleteActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Deleted"]}
       /\ model_step_count' = model_step_count + 1


schedule_DeletePaused(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Delete"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Paused"
       /\ schedule_phase' = "Deleted"
       /\ schedule_revision' = (schedule_revision) + 1
       /\ schedule_planning_cursor_utc_ms' = None
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "DeletePaused"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> "Deleted", revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeletePaused"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [at_utc_ms |-> packet.payload.at_utc_ms, superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeletePaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "DeletePaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Deleted"]}
       /\ model_step_count' = model_step_count + 1


schedule_DeleteDeleted(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Delete"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Deleted"
       /\ schedule_phase' = "Deleted"
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "DeleteDeleted", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Deleted"]}
       /\ model_step_count' = model_step_count + 1


schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "ConfirmOccurrencesSuperseded"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.superseding_revision = arg_superseding_revision
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Active"
       /\ schedule_superseded_ack_ids' = (schedule_superseded_ack_ids \cup {packet.payload.occurrence_id})
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "ConfirmOccurrencesSupersededActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "ConfirmOccurrencesSuperseded"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.superseding_revision = arg_superseding_revision
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Paused"
       /\ schedule_phase' = "Paused"
       /\ schedule_superseded_ack_ids' = (schedule_superseded_ack_ids \cup {packet.payload.occurrence_id})
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "ConfirmOccurrencesSupersededPaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Paused"]}
       /\ model_step_count' = model_step_count + 1


schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "ConfirmOccurrencesSuperseded"
       /\ packet.payload.occurrence_id = arg_occurrence_id
       /\ packet.payload.superseding_revision = arg_superseding_revision
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Deleted"
       /\ schedule_phase' = "Deleted"
       /\ schedule_superseded_ack_ids' = (schedule_superseded_ack_ids \cup {packet.payload.occurrence_id})
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "ConfirmOccurrencesSupersededDeleted", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Deleted"]}
       /\ model_step_count' = model_step_count + 1


schedule_revision_is_positive == (schedule_revision > 0)
schedule_deleted_has_no_planning_cursor == ((schedule_phase # "Deleted") \/ (schedule_planning_cursor_utc_ms = None))
schedule_planning_cursor_requires_occurrence_progress == ((schedule_planning_cursor_utc_ms = None) \/ (schedule_next_occurrence_ordinal > 0))

DeliverQueuedRoute ==
    /\ Len(pending_routes) > 0
    /\ LET route == Head(pending_routes) IN
       /\ pending_routes' = Tail(pending_routes)
       /\ delivered_routes' = delivered_routes \cup {route}
       /\ model_step_count' = model_step_count + 1
       /\ pending_inputs' = AppendIfMissing(pending_inputs, [machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> route.target_machine, variant |-> route.target_input, payload |-> route.payload, source_kind |-> "route", source_route |-> route.route, source_machine |-> route.source_machine, source_effect |-> route.effect, effect_id |-> route.effect_id]}
       /\ UNCHANGED << occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_trigger_key, occurrence_target_binding_key, occurrence_misfire_policy, occurrence_misfire_policy_key, occurrence_overlap_policy, occurrence_overlap_policy_key, occurrence_missing_target_policy, occurrence_missing_target_policy_key, occurrence_due_at_utc_ms, occurrence_misfire_deadline_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_target_materialized_session_id, occurrence_receipt_recorded_at_utc_ms, occurrence_last_receipt_recorded_at_utc_ms, occurrence_last_receipt_attempt, occurrence_last_receipt_stage, occurrence_last_receipt_failure_class, occurrence_last_receipt_detail, occurrence_last_receipt_correlation_id, occurrence_last_receipt_materialized_session_id, occurrence_runtime_outcome_key, occurrence_receipt_stage, occurrence_receipt_failure_class, occurrence_receipt_detail, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, schedule_phase, schedule_schedule_id, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_misfire_policy_key, schedule_overlap_policy, schedule_overlap_policy_key, schedule_missing_target_policy, schedule_missing_target_policy_key, schedule_planning_horizon_days, schedule_planning_horizon_occurrences, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_mob_delivery_feedback ==
    FALSE

WitnessInjectNext_materialization_failure_classification ==
    FALSE

WitnessInjectNext_revision_supersede_route ==
    FALSE

WitnessInjectNext_occurrence_supersede_ack_route ==
    FALSE

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalPending(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalClaimed(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDispatching(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalCompleted(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSkipped(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalMisfired(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSuperseded(arg_observation)
    \/ \E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(arg_observation)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : occurrence_PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingFuture(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingMisfire(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingClaimEligible(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseExpired(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseExpired(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseExpired(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseCurrent(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseCurrent(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseCurrent(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueCompletedNoAction(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSkippedNoAction(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueMisfiredNoAction(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSupersededNoAction(arg_now_utc_ms)
    \/ \E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDeliveryFailedNoAction(arg_now_utc_ms)
    \/ \E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id)
    \/ \E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptPending(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptClaimed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDispatching(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptAwaitingCompletion(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptCompleted(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSkipped(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptMisfired(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSuperseded(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDeliveryFailed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key)
    \/ \E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_at_utc_ms)
    \/ \E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionCompleted(arg_outcome, arg_detail, arg_at_utc_ms)
    \/ \E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionRuntimeRejected(arg_outcome, arg_detail, arg_at_utc_ms)
    \/ \E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionTransportError(arg_outcome, arg_detail, arg_at_utc_ms)
    \/ \E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionInternalError(arg_outcome, arg_detail, arg_at_utc_ms)
    \/ \E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)
    \/ \E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)
    \/ \E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)
    \/ \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_failure_class, arg_detail, arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromClaimed(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromDispatching(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromAwaitingCompletion(arg_at_utc_ms)
    \/ \E arg_schedule_id \in ScheduleIdValues : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in OptionU64Values : \E arg_planning_horizon_occurrences \in OptionU64Values : schedule_CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms)
    \/ \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms)
    \/ \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences)
    \/ \E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)
    \/ \E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotActive(arg_target_binding_key)
    \/ \E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotPaused(arg_target_binding_key)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision)
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

WitnessNext_revision_supersede_route ==
    \/ CoreNext
    \/ WitnessInjectNext_revision_supersede_route

WitnessNext_occurrence_supersede_ack_route ==
    \/ CoreNext
    \/ WitnessInjectNext_occurrence_supersede_ack_route


RouteObserved_revision_supersede_enters_occurrence_authority == \E packet \in RoutePackets : packet.route = "revision_supersede_enters_occurrence_authority"
RouteCoverage_revision_supersede_enters_occurrence_authority == (RouteObserved_revision_supersede_enters_occurrence_authority \/ ~RouteObserved_revision_supersede_enters_occurrence_authority)
RouteObserved_occurrence_supersede_ack_returns_to_schedule == \E packet \in RoutePackets : packet.route = "occurrence_supersede_ack_returns_to_schedule"
RouteCoverage_occurrence_supersede_ack_returns_to_schedule == (RouteObserved_occurrence_supersede_ack_returns_to_schedule \/ ~RouteObserved_occurrence_supersede_ack_returns_to_schedule)
CoverageInstrumentation == RouteCoverage_revision_supersede_enters_occurrence_authority /\ RouteCoverage_occurrence_supersede_ack_returns_to_schedule

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(schedule_superseded_ack_ids) <= 2
WitnessStateConstraint_mob_delivery_feedback == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0
WitnessStateConstraint_materialization_failure_classification == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0
WitnessStateConstraint_revision_supersede_route == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 11 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0
WitnessStateConstraint_occurrence_supersede_ack_route == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 11 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0

Spec ==
    /\ Init
    /\ [][Next]_vars

WitnessFairness_mob_delivery_feedback_1 ==
    /\ WF_vars(DeliverQueuedRoute)
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedCompleted(arg_observation))

WitnessFairness_mob_delivery_feedback_2 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedClaimed(arg_observation))

WitnessFairness_mob_delivery_feedback_3 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSuperseded(arg_observation))

WitnessFairness_mob_delivery_feedback_4 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : occurrence_PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingFuture(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingMisfire(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingClaimEligible(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueCompletedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSkippedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueMisfiredNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSupersededNoAction(arg_now_utc_ms))

WitnessFairness_mob_delivery_feedback_5 ==
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDeliveryFailedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptPending(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptClaimed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDispatching(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptAwaitingCompletion(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptCompleted(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSkipped(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptMisfired(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSuperseded(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDeliveryFailed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionCompleted(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionRuntimeRejected(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionTransportError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionInternalError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_failure_class, arg_detail, arg_at_utc_ms))

WitnessFairness_mob_delivery_feedback_6 ==
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_schedule_id \in ScheduleIdValues : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in OptionU64Values : \E arg_planning_horizon_occurrences \in OptionU64Values : schedule_CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotActive(arg_target_binding_key))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotPaused(arg_target_binding_key))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision))

WitnessSpec_mob_delivery_feedback ==
    /\ WitnessInit_mob_delivery_feedback
    /\ [] [WitnessNext_mob_delivery_feedback]_vars
    /\ WitnessFairness_mob_delivery_feedback_1
    /\ WitnessFairness_mob_delivery_feedback_2
    /\ WitnessFairness_mob_delivery_feedback_3
    /\ WitnessFairness_mob_delivery_feedback_4
    /\ WitnessFairness_mob_delivery_feedback_5
    /\ WitnessFairness_mob_delivery_feedback_6

WitnessFairness_materialization_failure_classification_1 ==
    /\ WF_vars(DeliverQueuedRoute)
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedCompleted(arg_observation))

WitnessFairness_materialization_failure_classification_2 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedClaimed(arg_observation))

WitnessFairness_materialization_failure_classification_3 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSuperseded(arg_observation))

WitnessFairness_materialization_failure_classification_4 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : occurrence_PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingFuture(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingMisfire(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingClaimEligible(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueCompletedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSkippedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueMisfiredNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSupersededNoAction(arg_now_utc_ms))

WitnessFairness_materialization_failure_classification_5 ==
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDeliveryFailedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptPending(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptClaimed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDispatching(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptAwaitingCompletion(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptCompleted(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSkipped(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptMisfired(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSuperseded(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDeliveryFailed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionCompleted(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionRuntimeRejected(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionTransportError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionInternalError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_failure_class, arg_detail, arg_at_utc_ms))

WitnessFairness_materialization_failure_classification_6 ==
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_schedule_id \in ScheduleIdValues : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in OptionU64Values : \E arg_planning_horizon_occurrences \in OptionU64Values : schedule_CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotActive(arg_target_binding_key))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotPaused(arg_target_binding_key))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision))

WitnessSpec_materialization_failure_classification ==
    /\ WitnessInit_materialization_failure_classification
    /\ [] [WitnessNext_materialization_failure_classification]_vars
    /\ WitnessFairness_materialization_failure_classification_1
    /\ WitnessFairness_materialization_failure_classification_2
    /\ WitnessFairness_materialization_failure_classification_3
    /\ WitnessFairness_materialization_failure_classification_4
    /\ WitnessFairness_materialization_failure_classification_5
    /\ WitnessFairness_materialization_failure_classification_6

WitnessFairness_revision_supersede_route_1 ==
    /\ WF_vars(DeliverQueuedRoute)
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedCompleted(arg_observation))

WitnessFairness_revision_supersede_route_2 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedClaimed(arg_observation))

WitnessFairness_revision_supersede_route_3 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSuperseded(arg_observation))

WitnessFairness_revision_supersede_route_4 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : occurrence_PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingFuture(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingMisfire(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingClaimEligible(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueCompletedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSkippedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueMisfiredNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSupersededNoAction(arg_now_utc_ms))

WitnessFairness_revision_supersede_route_5 ==
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDeliveryFailedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptPending(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptClaimed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDispatching(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptAwaitingCompletion(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptCompleted(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSkipped(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptMisfired(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSuperseded(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDeliveryFailed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionCompleted(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionRuntimeRejected(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionTransportError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionInternalError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_failure_class, arg_detail, arg_at_utc_ms))

WitnessFairness_revision_supersede_route_6 ==
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_schedule_id \in ScheduleIdValues : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in OptionU64Values : \E arg_planning_horizon_occurrences \in OptionU64Values : schedule_CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotActive(arg_target_binding_key))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotPaused(arg_target_binding_key))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision))

WitnessSpec_revision_supersede_route ==
    /\ WitnessInit_revision_supersede_route
    /\ [] [WitnessNext_revision_supersede_route]_vars
    /\ WitnessFairness_revision_supersede_route_1
    /\ WitnessFairness_revision_supersede_route_2
    /\ WitnessFairness_revision_supersede_route_3
    /\ WitnessFairness_revision_supersede_route_4
    /\ WitnessFairness_revision_supersede_route_5
    /\ WitnessFairness_revision_supersede_route_6

WitnessFairness_occurrence_supersede_ack_route_1 ==
    /\ WF_vars(DeliverQueuedRoute)
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailurePlanRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedCompleted(arg_observation))

WitnessFairness_occurrence_supersede_ack_route_2 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedClaimed(arg_observation))

WitnessFairness_occurrence_supersede_ack_route_3 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotClaimedDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotDispatchingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingSuperseded(arg_observation))

WitnessFairness_occurrence_supersede_ack_route_4 ==
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalPending(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalClaimed(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDispatching(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalCompleted(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSkipped(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalMisfired(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalSuperseded(arg_observation))
    /\ WF_vars(\E arg_observation \in OccurrenceTransitionFailureObservationKindValues : occurrence_ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(arg_observation))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : occurrence_PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingFuture(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingMisfire(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDuePendingClaimEligible(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseExpired(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueClaimedLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDispatchingLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueAwaitingCompletionLeaseCurrent(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueCompletedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSkippedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueMisfiredNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueSupersededNoAction(arg_now_utc_ms))

WitnessFairness_occurrence_supersede_ack_route_5 ==
    /\ WF_vars(\E arg_now_utc_ms \in 0..2 : occurrence_ClassifyDueDeliveryFailedNoAction(arg_now_utc_ms))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : occurrence_SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptPending(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptClaimed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDispatching(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptAwaitingCompletion(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptCompleted(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSkipped(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptMisfired(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptSuperseded(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_detail \in OptionStringValues : \E arg_materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : occurrence_RecordReceiptDeliveryFailed(arg_correlation_id, arg_detail, arg_materialized_session_id, arg_runtime_outcome_key))
    /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token))
    /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionCompleted(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionRuntimeRejected(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionTransportError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_outcome \in RuntimeCompletionOutcomeValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_RuntimeCompletionInternalError(arg_outcome, arg_detail, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms))
    /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms))
    /\ WF_vars(\E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_failure_class, arg_detail, arg_at_utc_ms))

WitnessFairness_occurrence_supersede_ack_route_6 ==
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromClaimed(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromDispatching(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_ReleaseLeaseForPausedScheduleFromAwaitingCompletion(arg_at_utc_ms))
    /\ WF_vars(\E arg_schedule_id \in ScheduleIdValues : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in OptionU64Values : \E arg_planning_horizon_occurrences \in OptionU64Values : schedule_CreateSchedule(arg_schedule_id, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : \E arg_at_utc_ms \in 0..2 : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_planning_horizon_days, arg_planning_horizon_occurrences, arg_at_utc_ms))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigActive(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_horizon_days \in 0..2 : \E arg_planning_horizon_occurrences \in 0..2 : schedule_UpdatePlanningConfigPaused(arg_planning_horizon_days, arg_planning_horizon_occurrences))
    /\ WF_vars(\E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotActive(arg_target_binding_key))
    /\ WF_vars(\E arg_target_binding_key \in StringValues : schedule_SyncTargetSnapshotPaused(arg_target_binding_key))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms))
    /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision))
    /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision))

WitnessSpec_occurrence_supersede_ack_route ==
    /\ WitnessInit_occurrence_supersede_ack_route
    /\ [] [WitnessNext_occurrence_supersede_ack_route]_vars
    /\ WitnessFairness_occurrence_supersede_ack_route_1
    /\ WitnessFairness_occurrence_supersede_ack_route_2
    /\ WitnessFairness_occurrence_supersede_ack_route_3
    /\ WitnessFairness_occurrence_supersede_ack_route_4
    /\ WitnessFairness_occurrence_supersede_ack_route_5
    /\ WitnessFairness_occurrence_supersede_ack_route_6

WitnessRouteObserved_revision_supersede_route_revision_supersede_enters_occurrence_authority == <> RouteObserved_revision_supersede_enters_occurrence_authority
WitnessRouteObserved_occurrence_supersede_ack_route_occurrence_supersede_ack_returns_to_schedule == <> RouteObserved_occurrence_supersede_ack_returns_to_schedule

THEOREM Spec => []occurrence_live_claim_requires_owner
THEOREM Spec => []occurrence_superseded_records_revision
THEOREM Spec => []occurrence_delivery_failed_records_failure_class
THEOREM Spec => []occurrence_misfire_deadline_not_before_due
THEOREM Spec => []schedule_revision_is_positive
THEOREM Spec => []schedule_deleted_has_no_planning_cursor
THEOREM Spec => []schedule_planning_cursor_requires_occurrence_progress

=============================================================================
