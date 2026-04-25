---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated composition model for schedule_bundle.

CONSTANTS ClaimTokenValues, DeliveryReceiptValues, MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, OverlapPolicyValues, ScheduleIdValues, SetOfOccurrenceIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionClaimTokenValues == {None} \cup {Some(x) : x \in ClaimTokenValues}
OptionDeliveryReceiptValues == {None} \cup {Some(x) : x \in DeliveryReceiptValues}
OptionOccurrenceFailureClassValues == {None} \cup {Some(x) : x \in OccurrenceFailureClassValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}
OptionU64Values == {None} \cup {Some(x) : x \in NatValues}

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
    <<"schedule", "ScheduleLifecycleMachine", "schedule_authority">>,
    <<"occurrence", "OccurrenceLifecycleMachine", "occurrence_authority">>
}

RouteNames == {
    "revision_supersede_enters_occurrence_authority",
    "occurrence_supersede_ack_returns_to_schedule"
}

Actors == {
    "schedule_authority",
    "occurrence_authority"
}

ActorPriorities == {
}

SchedulerRules == {
}

ActorOfMachine(machine_id) ==
    CASE machine_id = "schedule" -> "schedule_authority"
      [] machine_id = "occurrence" -> "occurrence_authority"
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

VARIABLES schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs
vars == << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, model_step_count, pending_inputs, observed_inputs, pending_routes, delivered_routes, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

RoutePackets == SeqElements(pending_routes) \cup delivered_routes
PendingActors == {ActorOfMachine(packet.machine) : packet \in SeqElements(pending_inputs)}
HigherPriorityReady(actor) == \E priority \in ActorPriorities : /\ priority[2] = actor /\ priority[1] \in PendingActors

BaseInit ==
    /\ schedule_phase = "Active"
    /\ schedule_revision = 1
    /\ schedule_trigger_key = "trigger-0"
    /\ schedule_target_binding_key = "target-0"
    /\ schedule_misfire_policy = "Skip"
    /\ schedule_overlap_policy = "SkipIfRunning"
    /\ schedule_missing_target_policy = "MarkMisfired"
    /\ schedule_planning_cursor_utc_ms = None
    /\ schedule_next_occurrence_ordinal = 0
    /\ schedule_superseded_ack_ids = {}
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
    /\ occurrence_last_receipt = None
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

WitnessInit_pause_resume_without_revision ==
    /\ BaseInit
    /\ pending_inputs = <<>>
    /\ observed_inputs = {}
    /\ witness_current_script_input = None
    /\ witness_remaining_script_inputs = <<>>

schedule_CreateSchedule(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Create"
       /\ packet.payload.trigger_key = arg_trigger_key
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.misfire_policy = arg_misfire_policy
       /\ packet.payload.overlap_policy = arg_overlap_policy
       /\ packet.payload.missing_target_policy = arg_missing_target_policy
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Active"
       /\ schedule_trigger_key' = packet.payload.trigger_key
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ schedule_misfire_policy' = packet.payload.misfire_policy
       /\ schedule_overlap_policy' = packet.payload.overlap_policy
       /\ schedule_missing_target_policy' = packet.payload.missing_target_policy
       /\ UNCHANGED << schedule_revision, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "CreateSchedule"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "CreateSchedule", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Revise"
       /\ packet.payload.trigger_key = arg_trigger_key
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.misfire_policy = arg_misfire_policy
       /\ packet.payload.overlap_policy = arg_overlap_policy
       /\ packet.payload.missing_target_policy = arg_missing_target_policy
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active"
       /\ schedule_phase' = "Active"
       /\ schedule_revision' = (schedule_revision) + 1
       /\ schedule_trigger_key' = packet.payload.trigger_key
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ schedule_misfire_policy' = packet.payload.misfire_policy
       /\ schedule_overlap_policy' = packet.payload.overlap_policy
       /\ schedule_missing_target_policy' = packet.payload.missing_target_policy
       /\ schedule_planning_cursor_utc_ms' = None
       /\ UNCHANGED << schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "ReviseActive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "ReviseActive"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "ReviseActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "ReviseActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Revise"
       /\ packet.payload.trigger_key = arg_trigger_key
       /\ packet.payload.target_binding_key = arg_target_binding_key
       /\ packet.payload.misfire_policy = arg_misfire_policy
       /\ packet.payload.overlap_policy = arg_overlap_policy
       /\ packet.payload.missing_target_policy = arg_missing_target_policy
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Paused"
       /\ schedule_phase' = "Paused"
       /\ schedule_revision' = (schedule_revision) + 1
       /\ schedule_trigger_key' = packet.payload.trigger_key
       /\ schedule_target_binding_key' = packet.payload.target_binding_key
       /\ schedule_misfire_policy' = packet.payload.misfire_policy
       /\ schedule_overlap_policy' = packet.payload.overlap_policy
       /\ schedule_missing_target_policy' = packet.payload.missing_target_policy
       /\ schedule_planning_cursor_utc_ms' = None
       /\ UNCHANGED << schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "RevisePaused"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "RevisePaused"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "RevisePaused"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "RevisePaused", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Paused"]}
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
       /\ UNCHANGED << schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "RecordPlanningWindowActive"], [machine |-> "schedule", variant |-> "PlanningWindowRecorded", payload |-> [next_occurrence_ordinal |-> packet.payload.next_occurrence_ordinal, planning_cursor_utc_ms |-> packet.payload.planning_cursor_utc_ms], effect_id |-> (model_step_count + 1), source_transition |-> "RecordPlanningWindowActive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "schedule", transition |-> "RecordPlanningWindowActive", actor |-> "schedule_authority", step |-> (model_step_count + 1), from_phase |-> schedule_phase, to_phase |-> "Active"]}
       /\ model_step_count' = model_step_count + 1


schedule_PauseActiveOrPaused(arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "schedule"
       /\ packet.variant = "Pause"
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("schedule_authority")
       /\ schedule_phase = "Active" \/ schedule_phase = "Paused"
       /\ schedule_phase' = "Paused"
       /\ UNCHANGED << schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "PauseActiveOrPaused"] }
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
       /\ UNCHANGED << schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> schedule_revision], effect_id |-> (model_step_count + 1), source_transition |-> "ResumeActiveOrPaused"] }
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
       /\ UNCHANGED << schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "DeleteActive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeleteActive"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeleteActive"] }
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
       /\ UNCHANGED << schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "occurrence", variant |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], source_kind |-> "route", source_route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", source_effect |-> "SupersedePendingOccurrences", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "revision_supersede_enters_occurrence_authority", source_machine |-> "schedule", effect |-> "SupersedePendingOccurrences", target_machine |-> "occurrence", target_input |-> "Supersede", payload |-> [at_utc_ms |-> 0, superseded_by_revision |-> (schedule_revision) + 1], actor |-> "occurrence_authority", effect_id |-> (model_step_count + 1), source_transition |-> "DeletePaused"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "schedule", variant |-> "EmitScheduleNotice", payload |-> [new_state |-> schedule_phase, revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeletePaused"], [machine |-> "schedule", variant |-> "SupersedePendingOccurrences", payload |-> [superseding_revision |-> (schedule_revision) + 1], effect_id |-> (model_step_count + 1), source_transition |-> "DeletePaused"] }
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
       /\ UNCHANGED << schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_last_receipt' = None
       /\ occurrence_failure_class' = None
       /\ occurrence_failure_detail' = None
       /\ occurrence_dispatched_at_utc_ms' = None
       /\ occurrence_completed_at_utc_ms' = None
       /\ occurrence_attempt_count' = (occurrence_attempt_count) + 1
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = SeqRemove(pending_inputs, packet)
       /\ observed_inputs' = observed_inputs
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "AwaitingCompletion", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "AwaitCompletionFromDispatching"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "AwaitCompletionFromDispatching", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "AwaitingCompletion"]}
       /\ model_step_count' = model_step_count + 1


occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "Complete"
       /\ packet.payload.receipt = arg_receipt
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "Completed"
       /\ occurrence_last_receipt' = Some(packet.payload.receipt)
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_claimed_by' = None
       /\ occurrence_lease_expires_at_utc_ms' = None
       /\ occurrence_claim_token' = None
       /\ occurrence_delivery_correlation_id' = None
       /\ occurrence_failure_class' = packet.payload.failure_class
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_last_receipt, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ occurrence_failure_class' = packet.payload.failure_class
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_last_receipt, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, witness_current_script_input, witness_remaining_script_inputs >>
       /\ pending_inputs' = AppendIfMissing(SeqRemove(pending_inputs, packet), [machine |-> "schedule", variant |-> "ConfirmOccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], source_kind |-> "route", source_route |-> "occurrence_supersede_ack_returns_to_schedule", source_machine |-> "occurrence", source_effect |-> "OccurrencesSuperseded", effect_id |-> (model_step_count + 1)])
       /\ observed_inputs' = observed_inputs \cup {[machine |-> "schedule", variant |-> "ConfirmOccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], source_kind |-> "route", source_route |-> "occurrence_supersede_ack_returns_to_schedule", source_machine |-> "occurrence", source_effect |-> "OccurrencesSuperseded", effect_id |-> (model_step_count + 1)]}
       /\ pending_routes' = pending_routes
       /\ delivered_routes' = delivered_routes \cup { [route |-> "occurrence_supersede_ack_returns_to_schedule", source_machine |-> "occurrence", effect |-> "OccurrencesSuperseded", target_machine |-> "schedule", target_input |-> "ConfirmOccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], actor |-> "schedule_authority", effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePendingOrLive"] }
       /\ emitted_effects' = emitted_effects \cup { [machine |-> "occurrence", variant |-> "Superseded", payload |-> [tag |-> "unit"], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePendingOrLive"], [machine |-> "occurrence", variant |-> "OccurrencesSuperseded", payload |-> [occurrence_id |-> occurrence_occurrence_id, superseding_revision |-> packet.payload.superseded_by_revision], effect_id |-> (model_step_count + 1), source_transition |-> "SupersedePendingOrLive"] }
       /\ observed_transitions' = observed_transitions \cup {[machine |-> "occurrence", transition |-> "SupersedePendingOrLive", actor |-> "occurrence_authority", step |-> (model_step_count + 1), from_phase |-> occurrence_phase, to_phase |-> "Superseded"]}
       /\ model_step_count' = model_step_count + 1


occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt, arg_failure_class, arg_detail, arg_at_utc_ms) ==
    /\ \E packet \in SeqElements(pending_inputs) :
       /\ packet.machine = "occurrence"
       /\ packet.variant = "DeliveryFailed"
       /\ packet.payload.receipt = arg_receipt
       /\ packet.payload.failure_class = arg_failure_class
       /\ packet.payload.detail = arg_detail
       /\ packet.payload.at_utc_ms = arg_at_utc_ms
       /\ ~HigherPriorityReady("occurrence_authority")
       /\ occurrence_phase = "Claimed" \/ occurrence_phase = "Dispatching" \/ occurrence_phase = "AwaitingCompletion"
       /\ occurrence_phase' = "DeliveryFailed"
       /\ occurrence_last_receipt' = packet.payload.receipt
       /\ occurrence_failure_class' = Some(packet.payload.failure_class)
       /\ occurrence_failure_detail' = packet.payload.detail
       /\ occurrence_completed_at_utc_ms' = Some(packet.payload.at_utc_ms)
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_dispatched_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, witness_current_script_input, witness_remaining_script_inputs >>
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
       /\ UNCHANGED << schedule_phase, schedule_revision, schedule_trigger_key, schedule_target_binding_key, schedule_misfire_policy, schedule_overlap_policy, schedule_missing_target_policy, schedule_planning_cursor_utc_ms, schedule_next_occurrence_ordinal, schedule_superseded_ack_ids, occurrence_phase, occurrence_occurrence_id, occurrence_schedule_id, occurrence_schedule_revision, occurrence_occurrence_ordinal, occurrence_target_binding_key, occurrence_due_at_utc_ms, occurrence_claimed_by, occurrence_lease_expires_at_utc_ms, occurrence_claimed_at_utc_ms, occurrence_claim_token, occurrence_delivery_correlation_id, occurrence_last_receipt, occurrence_failure_class, occurrence_failure_detail, occurrence_dispatched_at_utc_ms, occurrence_completed_at_utc_ms, occurrence_attempt_count, occurrence_superseded_by_revision, emitted_effects, observed_transitions, witness_current_script_input, witness_remaining_script_inputs >>

QuiescentStutter ==
    /\ Len(pending_routes) = 0
    /\ Len(pending_inputs) = 0
    /\ UNCHANGED vars

WitnessInjectNext_revision_supersede_route ==
    FALSE

WitnessInjectNext_occurrence_supersede_ack_route ==
    FALSE

WitnessInjectNext_pause_resume_without_revision ==
    FALSE

CoreNext ==
    \/ DeliverQueuedRoute
    \/ \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_CreateSchedule(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)
    \/ \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)
    \/ \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)
    \/ \E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision)
    \/ \E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)
    \/ \E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)
    \/ \E arg_receipt \in DeliveryReceiptValues : \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt, arg_at_utc_ms)
    \/ \E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)
    \/ \E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)
    \/ \E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)
    \/ \E arg_receipt \in OptionDeliveryReceiptValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt, arg_failure_class, arg_detail, arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)
    \/ \E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms)
    \/ QuiescentStutter

InjectNext ==
    FALSE

Next ==
    \/ CoreNext

WitnessNext_revision_supersede_route ==
    \/ CoreNext
    \/ WitnessInjectNext_revision_supersede_route

WitnessNext_occurrence_supersede_ack_route ==
    \/ CoreNext
    \/ WitnessInjectNext_occurrence_supersede_ack_route

WitnessNext_pause_resume_without_revision ==
    \/ CoreNext
    \/ WitnessInjectNext_pause_resume_without_revision


schedule_revision_supersede_route_present == \E route_name \in RouteNames : /\ RouteSource(route_name) = "schedule" /\ RouteEffect(route_name) = "SupersedePendingOccurrences" /\ RouteTargetMachine(route_name) = "occurrence" /\ RouteTargetInput(route_name) = "Supersede"
superseded_occurrence_originates_from_schedule_revision == \A input_packet \in observed_inputs : ((input_packet.machine = "occurrence" /\ input_packet.variant = "Supersede" /\ input_packet.source_route = "revision_supersede_enters_occurrence_authority") => (/\ input_packet.source_kind = "route" /\ input_packet.source_machine = "schedule" /\ input_packet.source_effect = "SupersedePendingOccurrences" /\ \E effect_packet \in emitted_effects : /\ effect_packet.machine = "schedule" /\ effect_packet.variant = "SupersedePendingOccurrences" /\ effect_packet.effect_id = input_packet.effect_id /\ \E route_packet \in RoutePackets : /\ route_packet.route = "revision_supersede_enters_occurrence_authority" /\ route_packet.source_machine = "schedule" /\ route_packet.effect = "SupersedePendingOccurrences" /\ route_packet.target_machine = "occurrence" /\ route_packet.target_input = "Supersede" /\ route_packet.effect_id = input_packet.effect_id /\ route_packet.payload = input_packet.payload))
occurrence_supersede_ack_route_present == \E route_name \in RouteNames : /\ RouteSource(route_name) = "occurrence" /\ RouteEffect(route_name) = "OccurrencesSuperseded" /\ RouteTargetMachine(route_name) = "schedule" /\ RouteTargetInput(route_name) = "ConfirmOccurrencesSuperseded"

RouteObserved_revision_supersede_enters_occurrence_authority == \E packet \in RoutePackets : packet.route = "revision_supersede_enters_occurrence_authority"
RouteCoverage_revision_supersede_enters_occurrence_authority == (RouteObserved_revision_supersede_enters_occurrence_authority \/ ~RouteObserved_revision_supersede_enters_occurrence_authority)
RouteObserved_occurrence_supersede_ack_returns_to_schedule == \E packet \in RoutePackets : packet.route = "occurrence_supersede_ack_returns_to_schedule"
RouteCoverage_occurrence_supersede_ack_returns_to_schedule == (RouteObserved_occurrence_supersede_ack_returns_to_schedule \/ ~RouteObserved_occurrence_supersede_ack_returns_to_schedule)
CoverageInstrumentation == RouteCoverage_revision_supersede_enters_occurrence_authority /\ RouteCoverage_occurrence_supersede_ack_returns_to_schedule

CiStateConstraint == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0
DeepStateConstraint == /\ model_step_count <= 6 /\ Len(pending_inputs) <= 2 /\ Cardinality(observed_inputs) <= 6 /\ Len(pending_routes) <= 2 /\ Cardinality(delivered_routes) <= 2 /\ Cardinality(emitted_effects) <= 2 /\ Cardinality(observed_transitions) <= 6 /\ Cardinality(schedule_superseded_ack_ids) <= 2
WitnessStateConstraint_revision_supersede_route == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 11 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0
WitnessStateConstraint_occurrence_supersede_ack_route == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 11 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 1 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0
WitnessStateConstraint_pause_resume_without_revision == /\ model_step_count <= 8 /\ Len(pending_inputs) <= 8 /\ Cardinality(observed_inputs) <= 10 /\ Len(pending_routes) <= 8 /\ Cardinality(delivered_routes) <= 0 /\ Cardinality(emitted_effects) <= 0 /\ Cardinality(observed_transitions) <= 8 /\ Cardinality(schedule_superseded_ack_ids) <= 0

Spec == Init /\ [][Next]_vars
WitnessSpec_revision_supersede_route == WitnessInit_revision_supersede_route /\ [] [WitnessNext_revision_supersede_route]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_CreateSchedule(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)) /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_receipt \in DeliveryReceiptValues : \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)) /\ WF_vars(\E arg_receipt \in OptionDeliveryReceiptValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt, arg_failure_class, arg_detail, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))
WitnessSpec_occurrence_supersede_ack_route == WitnessInit_occurrence_supersede_ack_route /\ [] [WitnessNext_occurrence_supersede_ack_route]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_CreateSchedule(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)) /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_receipt \in DeliveryReceiptValues : \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)) /\ WF_vars(\E arg_receipt \in OptionDeliveryReceiptValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt, arg_failure_class, arg_detail, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))
WitnessSpec_pause_resume_without_revision == WitnessInit_pause_resume_without_revision /\ [] [WitnessNext_pause_resume_without_revision]_vars /\ WF_vars(DeliverQueuedRoute) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_CreateSchedule(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_ReviseActive(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : schedule_RevisePaused(arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_overlap_policy, arg_missing_target_policy)) /\ WF_vars(\E arg_planning_cursor_utc_ms \in 0..2 : \E arg_next_occurrence_ordinal \in 0..2 : schedule_RecordPlanningWindowActive(arg_planning_cursor_utc_ms, arg_next_occurrence_ordinal)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_PauseActiveOrPaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_ResumeActiveOrPaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteActive(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeletePaused(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : schedule_DeleteDeleted(arg_at_utc_ms)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededActive(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededPaused(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_occurrence_id \in OccurrenceIdValues : \E arg_superseding_revision \in 0..2 : schedule_ConfirmOccurrencesSupersededDeleted(arg_occurrence_id, arg_superseding_revision)) /\ WF_vars(\E arg_owner_id \in StringValues : \E arg_at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : occurrence_ClaimPending(arg_owner_id, arg_at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)) /\ WF_vars(\E arg_correlation_id \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DispatchStartedFromClaimed(arg_correlation_id, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_AwaitCompletionFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_receipt \in DeliveryReceiptValues : \E arg_at_utc_ms \in 0..2 : occurrence_CompleteFromDispatchingOrAwaiting(arg_receipt, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_SkipFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E arg_at_utc_ms \in 0..2 : occurrence_MisfireFromPendingOrLive(arg_detail, arg_failure_class, arg_at_utc_ms)) /\ WF_vars(\E arg_superseded_by_revision \in 0..2 : \E arg_at_utc_ms \in 0..2 : occurrence_SupersedePendingOrLive(arg_superseded_by_revision, arg_at_utc_ms)) /\ WF_vars(\E arg_receipt \in OptionDeliveryReceiptValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E arg_detail \in OptionStringValues : \E arg_at_utc_ms \in 0..2 : occurrence_DeliveryFailedFromClaimedOrLive(arg_receipt, arg_failure_class, arg_detail, arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromClaimed(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromDispatching(arg_at_utc_ms)) /\ WF_vars(\E arg_at_utc_ms \in 0..2 : occurrence_LeaseExpiredFromAwaitingCompletion(arg_at_utc_ms))

WitnessRouteObserved_revision_supersede_route_revision_supersede_enters_occurrence_authority == <> RouteObserved_revision_supersede_enters_occurrence_authority
WitnessRouteObserved_occurrence_supersede_ack_route_occurrence_supersede_ack_returns_to_schedule == <> RouteObserved_occurrence_supersede_ack_returns_to_schedule

THEOREM Spec => []schedule_revision_supersede_route_present
THEOREM Spec => []superseded_occurrence_originates_from_schedule_revision
THEOREM Spec => []occurrence_supersede_ack_route_present
THEOREM Spec => []schedule_revision_is_positive
THEOREM Spec => []schedule_deleted_has_no_planning_cursor
THEOREM Spec => []schedule_planning_cursor_requires_occurrence_progress
THEOREM Spec => []occurrence_live_claim_requires_owner
THEOREM Spec => []occurrence_superseded_records_revision
THEOREM Spec => []occurrence_delivery_failed_records_failure_class

=============================================================================
