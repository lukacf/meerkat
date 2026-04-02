---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OccurrenceLifecycleMachine.

CONSTANTS NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, lease_held, lease_owner, lease_expiry_utc_ms, delivery_correlation_id, open_delivery_protocol, last_receipt_stage, failure_class, attempt_count, superseded_by_revision

vars == << phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, lease_held, lease_owner, lease_expiry_utc_ms, delivery_correlation_id, open_delivery_protocol, last_receipt_stage, failure_class, attempt_count, superseded_by_revision >>

phase_is_terminal(arg_phase) == ((arg_phase = "Completed") \/ (arg_phase = "Skipped") \/ (arg_phase = "Misfired") \/ (arg_phase = "Superseded") \/ (arg_phase = "DeliveryFailed"))
claimable_at(store_now_utc_ms) == ((phase = "Pending") /\ (due_at_utc_ms <= store_now_utc_ms) /\ (~(lease_held) \/ (lease_expiry_utc_ms <= store_now_utc_ms)))

Init ==
    /\ phase = "Pending"
    /\ model_step_count = 0
    /\ occurrence_id = "occurrence-0"
    /\ schedule_id = "schedule-0"
    /\ schedule_revision = 1
    /\ occurrence_ordinal = 0
    /\ target_binding_key = "target-0"
    /\ due_at_utc_ms = 1
    /\ lease_held = FALSE
    /\ lease_owner = ""
    /\ lease_expiry_utc_ms = 0
    /\ delivery_correlation_id = None
    /\ open_delivery_protocol = None
    /\ last_receipt_stage = Some("Planned")
    /\ failure_class = None
    /\ attempt_count = 0
    /\ superseded_by_revision = None

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Skipped" \/ phase = "Misfired" \/ phase = "Superseded" \/ phase = "DeliveryFailed"
    /\ UNCHANGED vars

ClaimPending(claim_time_utc_ms, owner_id, arg_lease_expiry_utc_ms) ==
    /\ phase = "Pending"
    /\ claimable_at(claim_time_utc_ms)
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = TRUE
    /\ lease_owner' = owner_id
    /\ lease_expiry_utc_ms' = arg_lease_expiry_utc_ms
    /\ delivery_correlation_id' = None
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Claimed")
    /\ failure_class' = None
    /\ attempt_count' = (attempt_count) + 1
    /\ superseded_by_revision' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms >>


StartRuntimeDispatchFromClaimed(arg_delivery_correlation_id) ==
    /\ phase = "Claimed"
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_correlation_id' = Some(arg_delivery_correlation_id)
    /\ open_delivery_protocol' = Some("Runtime")
    /\ last_receipt_stage' = Some("DispatchStarted")
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, lease_held, lease_owner, lease_expiry_utc_ms, failure_class, attempt_count, superseded_by_revision >>


StartMobDispatchFromClaimed(arg_delivery_correlation_id) ==
    /\ phase = "Claimed"
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_correlation_id' = Some(arg_delivery_correlation_id)
    /\ open_delivery_protocol' = Some("Mob")
    /\ last_receipt_stage' = Some("DispatchStarted")
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, lease_held, lease_owner, lease_expiry_utc_ms, failure_class, attempt_count, superseded_by_revision >>


RuntimeAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count) ==
    /\ phase = "Dispatching"
    /\ (open_delivery_protocol = Some("Runtime"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_stage' = Some("AwaitingCompletion")
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, lease_held, lease_owner, lease_expiry_utc_ms, delivery_correlation_id, open_delivery_protocol, failure_class, attempt_count, superseded_by_revision >>


MobAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count) ==
    /\ phase = "Dispatching"
    /\ (open_delivery_protocol = Some("Mob"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_stage' = Some("AwaitingCompletion")
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, lease_held, lease_owner, lease_expiry_utc_ms, delivery_correlation_id, open_delivery_protocol, failure_class, attempt_count, superseded_by_revision >>


RuntimeCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Runtime"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Completed")
    /\ failure_class' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


MobCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Mob"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Completed")
    /\ failure_class' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


RuntimeSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Runtime"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Skipped")
    /\ failure_class' = Some(arg_failure_class)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


MobSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Mob"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Skipped")
    /\ failure_class' = Some(arg_failure_class)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


RuntimeMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Runtime"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Misfired")
    /\ failure_class' = Some(arg_failure_class)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


MobMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Mob"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Misfired")
    /\ failure_class' = Some(arg_failure_class)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


RuntimeDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Runtime"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("DeliveryFailed")
    /\ failure_class' = Some(arg_failure_class)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


MobDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (open_delivery_protocol = Some("Mob"))
    /\ (occurrence_id = arg_occurrence_id)
    /\ (attempt_count = arg_attempt_count)
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("DeliveryFailed")
    /\ failure_class' = Some(arg_failure_class)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, delivery_correlation_id, attempt_count, superseded_by_revision >>


SupersedePending(superseding_revision) ==
    /\ phase = "Pending"
    /\ (superseding_revision > schedule_revision)
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("Superseded")
    /\ superseded_by_revision' = Some(superseding_revision)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, lease_held, lease_owner, lease_expiry_utc_ms, delivery_correlation_id, failure_class, attempt_count >>


LeaseExpiredFromClaimed ==
    /\ phase = "Claimed"
    /\ (lease_held = TRUE)
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ delivery_correlation_id' = None
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("LeaseExpired")
    /\ failure_class' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromDispatching ==
    /\ phase = "Dispatching"
    /\ (lease_held = TRUE)
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ delivery_correlation_id' = None
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("LeaseExpired")
    /\ failure_class' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromAwaitingCompletion ==
    /\ phase = "AwaitingCompletion"
    /\ (lease_held = TRUE)
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ lease_held' = FALSE
    /\ lease_owner' = ""
    /\ lease_expiry_utc_ms' = 0
    /\ delivery_correlation_id' = None
    /\ open_delivery_protocol' = None
    /\ last_receipt_stage' = Some("LeaseExpired")
    /\ failure_class' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, attempt_count, superseded_by_revision >>


Next ==
    \/ \E claim_time_utc_ms \in 0..2 : \E owner_id \in {"alpha", "beta"} : \E arg_lease_expiry_utc_ms \in 0..2 : ClaimPending(claim_time_utc_ms, owner_id, arg_lease_expiry_utc_ms)
    \/ \E arg_delivery_correlation_id \in {"alpha", "beta"} : StartRuntimeDispatchFromClaimed(arg_delivery_correlation_id)
    \/ \E arg_delivery_correlation_id \in {"alpha", "beta"} : StartMobDispatchFromClaimed(arg_delivery_correlation_id)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : RuntimeAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : MobAcceptedFromDispatching(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : RuntimeCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : MobCompletedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : RuntimeSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : MobSkippedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : RuntimeMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : MobMisfiredFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : RuntimeDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_attempt_count \in 0..2 : \E arg_failure_class \in OccurrenceFailureClassValues : MobDeliveryFailedFromDispatchingOrAwaiting(arg_occurrence_id, arg_attempt_count, arg_failure_class)
    \/ \E superseding_revision \in 0..2 : SupersedePending(superseding_revision)
    \/ LeaseExpiredFromClaimed
    \/ LeaseExpiredFromDispatching
    \/ LeaseExpiredFromAwaitingCompletion
    \/ TerminalStutter

terminal_has_no_open_delivery_protocol == (~(phase_is_terminal(phase)) \/ (open_delivery_protocol = None))
live_claim_requires_lease_holder == (((phase # "Claimed") /\ (phase # "Dispatching") /\ (phase # "AwaitingCompletion")) \/ (lease_held = TRUE))
awaiting_completion_requires_protocol == ((phase # "AwaitingCompletion") \/ (open_delivery_protocol # None))
superseded_records_revision == ((phase # "Superseded") \/ (superseded_by_revision # None))
delivery_failed_records_failure_class == ((phase # "DeliveryFailed") \/ (failure_class # None))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []terminal_has_no_open_delivery_protocol
THEOREM Spec => []live_claim_requires_lease_holder
THEOREM Spec => []awaiting_completion_requires_protocol
THEOREM Spec => []superseded_records_revision
THEOREM Spec => []delivery_failed_records_failure_class

=============================================================================
