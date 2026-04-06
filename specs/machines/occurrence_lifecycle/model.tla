---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OccurrenceLifecycleMachine.

CONSTANTS DeliveryReceiptStageValues, NatValues, OccurrenceFailureClassValues, StringValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionDeliveryReceiptStageValues == {None} \cup {Some(x) : x \in DeliveryReceiptStageValues}
OptionOccurrenceFailureClassValues == {None} \cup {Some(x) : x \in OccurrenceFailureClassValues}
OptionStringValues == {None} \cup {Some(x) : x \in StringValues}

MapLookup(map, key) == IF key \in DOMAIN map THEN map[key] ELSE None
MapSet(map, key, value) == [x \in DOMAIN map \cup {key} |-> IF x = key THEN value ELSE map[x]]
StartsWith(seq, prefix) == /\ Len(prefix) <= Len(seq) /\ SubSeq(seq, 1, Len(prefix)) = prefix
SeqElements(seq) == {seq[i] : i \in 1..Len(seq)}
RECURSIVE SeqRemove(_, _)
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, last_receipt_stage, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision

vars == << phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, last_receipt_stage, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>

is_live_claim_phase(arg_phase) == ((arg_phase = "Claimed") \/ (arg_phase = "Dispatching") \/ (arg_phase = "AwaitingCompletion"))

Init ==
    /\ phase = "Pending"
    /\ model_step_count = 0
    /\ occurrence_id = "occurrence-0"
    /\ schedule_id = "schedule-0"
    /\ schedule_revision = 1
    /\ occurrence_ordinal = 0
    /\ target_binding_key = "target-0"
    /\ due_at_utc_ms = 1
    /\ claimed_by = None
    /\ lease_expires_at_utc_ms = None
    /\ claimed_at_utc_ms = None
    /\ claim_token = None
    /\ delivery_correlation_id = None
    /\ last_receipt_stage = None
    /\ failure_class = None
    /\ failure_detail = None
    /\ dispatched_at_utc_ms = None
    /\ completed_at_utc_ms = None
    /\ attempt_count = 0
    /\ superseded_by_revision = None

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Skipped" \/ phase = "Misfired" \/ phase = "Superseded" \/ phase = "DeliveryFailed"
    /\ UNCHANGED vars

ClaimPending(owner_id, at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token) ==
    /\ phase = "Pending"
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = Some(owner_id)
    /\ lease_expires_at_utc_ms' = Some(arg_lease_expires_at_utc_ms)
    /\ claimed_at_utc_ms' = Some(at_utc_ms)
    /\ claim_token' = Some(arg_claim_token)
    /\ delivery_correlation_id' = None
    /\ last_receipt_stage' = None
    /\ failure_class' = None
    /\ failure_detail' = None
    /\ dispatched_at_utc_ms' = None
    /\ completed_at_utc_ms' = None
    /\ attempt_count' = (attempt_count) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, superseded_by_revision >>


DispatchStartedFromClaimed(correlation_id, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_correlation_id' = correlation_id
    /\ dispatched_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, last_receipt_stage, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


AwaitCompletionFromDispatching(at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ dispatched_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, last_receipt_stage, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


CompleteFromDispatchingOrAwaiting(receipt_stage, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_stage' = Some(receipt_stage)
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


SkipFromPendingOrLive(detail, arg_failure_class, at_utc_ms) ==
    /\ phase = "Pending" \/ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ failure_class' = arg_failure_class
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, last_receipt_stage, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


MisfireFromPendingOrLive(detail, arg_failure_class, at_utc_ms) ==
    /\ phase = "Pending" \/ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ failure_class' = arg_failure_class
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, last_receipt_stage, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


SupersedePendingOrLive(arg_superseded_by_revision, at_utc_ms) ==
    /\ phase = "Pending" \/ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ superseded_by_revision' = Some(arg_superseded_by_revision)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, last_receipt_stage, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count >>


DeliveryFailedFromClaimedOrLive(receipt_stage, arg_failure_class, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_stage' = receipt_stage
    /\ failure_class' = Some(arg_failure_class)
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromClaimed(at_utc_ms) ==
    /\ phase = "Claimed"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, last_receipt_stage, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromDispatching(at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, last_receipt_stage, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromAwaitingCompletion(at_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, target_binding_key, due_at_utc_ms, last_receipt_stage, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


Next ==
    \/ \E owner_id \in {"alpha", "beta"} : \E at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in {"alpha", "beta"} : ClaimPending(owner_id, at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)
    \/ \E correlation_id \in OptionStringValues : \E at_utc_ms \in 0..2 : DispatchStartedFromClaimed(correlation_id, at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : AwaitCompletionFromDispatching(at_utc_ms)
    \/ \E receipt_stage \in DeliveryReceiptStageValues : \E at_utc_ms \in 0..2 : CompleteFromDispatchingOrAwaiting(receipt_stage, at_utc_ms)
    \/ \E detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E at_utc_ms \in 0..2 : SkipFromPendingOrLive(detail, arg_failure_class, at_utc_ms)
    \/ \E detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E at_utc_ms \in 0..2 : MisfireFromPendingOrLive(detail, arg_failure_class, at_utc_ms)
    \/ \E arg_superseded_by_revision \in 0..2 : \E at_utc_ms \in 0..2 : SupersedePendingOrLive(arg_superseded_by_revision, at_utc_ms)
    \/ \E receipt_stage \in OptionDeliveryReceiptStageValues : \E arg_failure_class \in OccurrenceFailureClassValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailedFromClaimedOrLive(receipt_stage, arg_failure_class, detail, at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromClaimed(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromDispatching(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromAwaitingCompletion(at_utc_ms)
    \/ TerminalStutter

live_claim_requires_owner == (~(is_live_claim_phase(phase)) \/ (claimed_by # None))
superseded_records_revision == ((phase # "Superseded") \/ (superseded_by_revision # None))
delivery_failed_records_failure_class == ((phase # "DeliveryFailed") \/ (failure_class # None))

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []live_claim_requires_owner
THEOREM Spec => []superseded_records_revision
THEOREM Spec => []delivery_failed_records_failure_class

=============================================================================
