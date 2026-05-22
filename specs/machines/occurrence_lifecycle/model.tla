---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OccurrenceLifecycleMachine.

CONSTANTS ClaimTokenValues, DeliveryReceiptStageValues, MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, OccurrenceLifecycleInputVariantValues, OccurrenceTransitionFailureRefusalKindValues, OverlapPolicyValues, RuntimeCompletionOutcomeValues, ScheduleIdValues, SessionIdValues, StringValues

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
SeqRemove(seq, value) == IF Len(seq) = 0 THEN <<>> ELSE IF Head(seq) = value THEN SeqRemove(Tail(seq), value) ELSE <<Head(seq)>> \o SeqRemove(Tail(seq), value)
RECURSIVE SeqRemoveAll(_, _)
SeqRemoveAll(seq, values) == IF Len(values) = 0 THEN seq ELSE SeqRemoveAll(SeqRemove(seq, Head(values)), Tail(values))

VARIABLES phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision

vars == << phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>

is_live_claim_phase(arg_phase) == ((arg_phase = "Claimed") \/ (arg_phase = "Dispatching") \/ (arg_phase = "AwaitingCompletion"))

Init ==
    /\ phase = "Pending"
    /\ model_step_count = 0
    /\ occurrence_id = "occurrence-0"
    /\ schedule_id = "schedule-0"
    /\ schedule_revision = 1
    /\ occurrence_ordinal = 0
    /\ trigger_key = "trigger-0"
    /\ target_binding_key = "target-0"
    /\ misfire_policy = "Skip"
    /\ misfire_policy_key = "misfire:skip"
    /\ overlap_policy = "SkipIfRunning"
    /\ overlap_policy_key = "overlap:skip_if_running"
    /\ missing_target_policy = "MarkMisfired"
    /\ missing_target_policy_key = "missing_target:mark_misfired"
    /\ due_at_utc_ms = 1
    /\ misfire_deadline_utc_ms = 1
    /\ claimed_by = None
    /\ lease_expires_at_utc_ms = None
    /\ claimed_at_utc_ms = None
    /\ claim_token = None
    /\ delivery_correlation_id = None
    /\ target_materialized_session_id = None
    /\ receipt_recorded_at_utc_ms = None
    /\ last_receipt_recorded_at_utc_ms = None
    /\ last_receipt_attempt = None
    /\ last_receipt_stage = None
    /\ last_receipt_failure_class = None
    /\ last_receipt_detail = None
    /\ last_receipt_correlation_id = None
    /\ last_receipt_materialized_session_id = None
    /\ runtime_outcome_key = None
    /\ receipt_stage = None
    /\ receipt_failure_class = None
    /\ receipt_detail = None
    /\ failure_class = None
    /\ failure_detail = None
    /\ dispatched_at_utc_ms = None
    /\ completed_at_utc_ms = None
    /\ attempt_count = 0
    /\ superseded_by_revision = None

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Skipped" \/ phase = "Misfired" \/ phase = "Superseded" \/ phase = "DeliveryFailed"
    /\ UNCHANGED vars

ClassifyTransitionFailurePlanRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "PlanOccurrence") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "SyncTargetSnapshot") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "RecordReceipt") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyDue") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureClaimRejectedPendingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "Claim") /\ (refusal_kind = "GuardRejected"))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "Claim") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "DispatchStarted") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "AwaitCompletion") /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ (((trigger = "LeaseExpired") \/ (trigger = "ReleaseLeaseForPausedSchedule")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ (((trigger = "Complete") \/ (trigger = "ResolveRuntimeCompletion") \/ (trigger = "Skip") \/ (trigger = "Misfire") \/ (trigger = "Supersede") \/ (trigger = "DeliveryFailed")) /\ ((refusal_kind = "GuardRejected") \/ (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms) ==
    /\ phase = "Pending"
    /\ ((attempt_count = 0) /\ (claimed_by = None) /\ (claim_token = None) /\ (delivery_correlation_id = None) /\ (target_materialized_session_id = None) /\ (completed_at_utc_ms = None) /\ (superseded_by_revision = None) /\ (arg_misfire_deadline_utc_ms >= arg_due_at_utc_ms))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ occurrence_id' = arg_occurrence_id
    /\ schedule_id' = arg_schedule_id
    /\ schedule_revision' = arg_schedule_revision
    /\ occurrence_ordinal' = arg_occurrence_ordinal
    /\ trigger_key' = arg_trigger_key
    /\ target_binding_key' = arg_target_binding_key
    /\ misfire_policy' = arg_misfire_policy
    /\ misfire_policy_key' = arg_misfire_policy_key
    /\ overlap_policy' = arg_overlap_policy
    /\ overlap_policy_key' = arg_overlap_policy_key
    /\ missing_target_policy' = arg_missing_target_policy
    /\ missing_target_policy_key' = arg_missing_target_policy_key
    /\ due_at_utc_ms' = arg_due_at_utc_ms
    /\ misfire_deadline_utc_ms' = arg_misfire_deadline_utc_ms
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ target_materialized_session_id' = arg_target_materialized_session_id
    /\ receipt_recorded_at_utc_ms' = None
    /\ last_receipt_recorded_at_utc_ms' = None
    /\ last_receipt_attempt' = None
    /\ last_receipt_stage' = None
    /\ last_receipt_failure_class' = None
    /\ last_receipt_detail' = None
    /\ last_receipt_correlation_id' = None
    /\ last_receipt_materialized_session_id' = None
    /\ runtime_outcome_key' = None
    /\ receipt_stage' = None
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ failure_class' = None
    /\ failure_detail' = None
    /\ dispatched_at_utc_ms' = None
    /\ completed_at_utc_ms' = None
    /\ attempt_count' = 0
    /\ superseded_by_revision' = None


ClassifyDuePendingFuture(now_utc_ms) ==
    /\ phase = "Pending"
    /\ (now_utc_ms < due_at_utc_ms)
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDuePendingMisfire(now_utc_ms) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= now_utc_ms) /\ (misfire_deadline_utc_ms < now_utc_ms))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDuePendingClaimEligible(now_utc_ms) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= now_utc_ms) /\ (now_utc_ms <= misfire_deadline_utc_ms))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueClaimedLeaseExpired(now_utc_ms) ==
    /\ phase = "Claimed"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueDispatchingLeaseExpired(now_utc_ms) ==
    /\ phase = "Dispatching"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueAwaitingCompletionLeaseExpired(now_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueClaimedLeaseCurrent(now_utc_ms) ==
    /\ phase = "Claimed"
    /\ ((lease_expires_at_utc_ms = None) \/ (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueDispatchingLeaseCurrent(now_utc_ms) ==
    /\ phase = "Dispatching"
    /\ ((lease_expires_at_utc_ms = None) \/ (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueAwaitingCompletionLeaseCurrent(now_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ ((lease_expires_at_utc_ms = None) \/ (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueCompletedNoAction(now_utc_ms) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueSkippedNoAction(now_utc_ms) ==
    /\ phase = "Skipped"
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueMisfiredNoAction(now_utc_ms) ==
    /\ phase = "Misfired"
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueSupersededNoAction(now_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyDueDeliveryFailedNoAction(now_utc_ms) ==
    /\ phase = "DeliveryFailed"
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ phase = "Pending"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ target_materialized_session_id' = arg_target_materialized_session_id
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ phase = "Claimed"
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ target_materialized_session_id' = arg_target_materialized_session_id
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptPending(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "Pending"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptClaimed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "Claimed"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptDispatching(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "Dispatching"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptAwaitingCompletion(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "AwaitingCompletion"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptCompleted(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "Completed"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptSkipped(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "Skipped"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptMisfired(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "Misfired"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptSuperseded(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "Superseded"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


RecordReceiptDeliveryFailed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key) ==
    /\ phase = "DeliveryFailed"
    /\ ((receipt_stage # None) /\ (receipt_recorded_at_utc_ms # None) /\ (receipt_detail = detail) /\ (delivery_correlation_id = correlation_id) /\ (target_materialized_session_id = materialized_session_id))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ last_receipt_recorded_at_utc_ms' = receipt_recorded_at_utc_ms
    /\ last_receipt_attempt' = Some(attempt_count)
    /\ last_receipt_stage' = receipt_stage
    /\ last_receipt_failure_class' = receipt_failure_class
    /\ last_receipt_detail' = detail
    /\ last_receipt_correlation_id' = correlation_id
    /\ last_receipt_materialized_session_id' = materialized_session_id
    /\ runtime_outcome_key' = arg_runtime_outcome_key
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClaimPending(owner_id, at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= at_utc_ms) /\ (at_utc_ms <= misfire_deadline_utc_ms))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = Some(owner_id)
    /\ lease_expires_at_utc_ms' = Some(arg_lease_expires_at_utc_ms)
    /\ claimed_at_utc_ms' = Some(at_utc_ms)
    /\ claim_token' = Some(arg_claim_token)
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = None
    /\ last_receipt_recorded_at_utc_ms' = None
    /\ last_receipt_attempt' = None
    /\ last_receipt_stage' = None
    /\ last_receipt_failure_class' = None
    /\ last_receipt_detail' = None
    /\ last_receipt_correlation_id' = None
    /\ last_receipt_materialized_session_id' = None
    /\ runtime_outcome_key' = None
    /\ receipt_stage' = None
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ failure_class' = None
    /\ failure_detail' = None
    /\ dispatched_at_utc_ms' = None
    /\ completed_at_utc_ms' = None
    /\ attempt_count' = (attempt_count) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, superseded_by_revision >>


DispatchStartedFromClaimed(correlation_id, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ delivery_correlation_id' = correlation_id
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DispatchStarted")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ dispatched_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


AwaitCompletionFromDispatching(at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ dispatched_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


CompleteFromDispatchingOrAwaiting(at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Completed")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


RuntimeCompletionCompleted(outcome, detail, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (outcome = "Completed")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Completed")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


RuntimeCompletionRuntimeRejected(outcome, detail, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ ((outcome = "CallbackPending") \/ (outcome = "Cancelled") \/ (outcome = "Abandoned"))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("RuntimeRejected")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("RuntimeRejected")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


RuntimeCompletionTransportError(outcome, detail, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (outcome = "RuntimeTerminated")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("TransportError")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TransportError")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


RuntimeCompletionInternalError(outcome, detail, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (outcome = "FinalizationFailed")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("InternalError")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("InternalError")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


SkipFromPendingOrLive(detail, arg_failure_class, at_utc_ms) ==
    /\ phase = "Pending" \/ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Skipped")
    /\ receipt_failure_class' = arg_failure_class
    /\ receipt_detail' = detail
    /\ failure_class' = arg_failure_class
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_at_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


MisfireFromPendingOrLive(detail, arg_failure_class, at_utc_ms) ==
    /\ phase = "Pending" \/ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Misfired")
    /\ receipt_failure_class' = arg_failure_class
    /\ receipt_detail' = detail
    /\ failure_class' = arg_failure_class
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_at_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


SupersedePendingOrLive(arg_superseded_by_revision, at_utc_ms) ==
    /\ phase = "Pending" \/ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Superseded")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ superseded_by_revision' = Some(arg_superseded_by_revision)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count >>


DeliveryFailedFromClaimedOrLive(arg_failure_class, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some(arg_failure_class)
    /\ receipt_detail' = detail
    /\ failure_class' = Some(arg_failure_class)
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromClaimed(at_utc_ms) ==
    /\ phase = "Claimed"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("LeaseExpired")
    /\ receipt_failure_class' = Some("LeaseLost")
    /\ receipt_detail' = Some("lease expired before completion")
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromDispatching(at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("LeaseExpired")
    /\ receipt_failure_class' = Some("LeaseLost")
    /\ receipt_detail' = Some("lease expired before completion")
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


LeaseExpiredFromAwaitingCompletion(at_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("LeaseExpired")
    /\ receipt_failure_class' = Some("LeaseLost")
    /\ receipt_detail' = Some("lease expired before completion")
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ReleaseLeaseForPausedScheduleFromClaimed(at_utc_ms) ==
    /\ phase = "Claimed"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("LeaseExpired")
    /\ receipt_failure_class' = Some("LeaseLost")
    /\ receipt_detail' = Some("lease released because schedule was paused before dispatch")
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ReleaseLeaseForPausedScheduleFromDispatching(at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("LeaseExpired")
    /\ receipt_failure_class' = Some("LeaseLost")
    /\ receipt_detail' = Some("lease released because schedule was paused before dispatch")
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ReleaseLeaseForPausedScheduleFromAwaitingCompletion(at_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claimed_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("LeaseExpired")
    /\ receipt_failure_class' = Some("LeaseLost")
    /\ receipt_detail' = Some("lease released because schedule was paused before dispatch")
    /\ dispatched_at_utc_ms' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision >>


Next ==
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimRejectedPendingPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(refusal_kind, trigger)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in StringValues : \E arg_target_binding_key \in StringValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDuePendingFuture(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDuePendingMisfire(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDuePendingClaimEligible(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueClaimedLeaseExpired(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueDispatchingLeaseExpired(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueAwaitingCompletionLeaseExpired(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueClaimedLeaseCurrent(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueDispatchingLeaseCurrent(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueAwaitingCompletionLeaseCurrent(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueCompletedNoAction(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueSkippedNoAction(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueMisfiredNoAction(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueSupersededNoAction(now_utc_ms)
    \/ \E now_utc_ms \in 0..2 : ClassifyDueDeliveryFailedNoAction(now_utc_ms)
    \/ \E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id)
    \/ \E arg_target_binding_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptPending(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptClaimed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptDispatching(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptAwaitingCompletion(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptCompleted(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptSkipped(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptMisfired(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptSuperseded(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionStringValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionStringValues : RecordReceiptDeliveryFailed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E owner_id \in StringValues : \E at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : ClaimPending(owner_id, at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)
    \/ \E correlation_id \in OptionStringValues : \E at_utc_ms \in 0..2 : DispatchStartedFromClaimed(correlation_id, at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : AwaitCompletionFromDispatching(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : CompleteFromDispatchingOrAwaiting(at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionCompleted(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionRuntimeRejected(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionTransportError(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionInternalError(outcome, detail, at_utc_ms)
    \/ \E detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E at_utc_ms \in 0..2 : SkipFromPendingOrLive(detail, arg_failure_class, at_utc_ms)
    \/ \E detail \in OptionStringValues : \E arg_failure_class \in OptionOccurrenceFailureClassValues : \E at_utc_ms \in 0..2 : MisfireFromPendingOrLive(detail, arg_failure_class, at_utc_ms)
    \/ \E arg_superseded_by_revision \in 0..2 : \E at_utc_ms \in 0..2 : SupersedePendingOrLive(arg_superseded_by_revision, at_utc_ms)
    \/ \E arg_failure_class \in OccurrenceFailureClassValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailedFromClaimedOrLive(arg_failure_class, detail, at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromClaimed(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromDispatching(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromAwaitingCompletion(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromClaimed(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromDispatching(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromAwaitingCompletion(at_utc_ms)
    \/ TerminalStutter

live_claim_requires_owner == (~(is_live_claim_phase(phase)) \/ (claimed_by # None))
superseded_records_revision == ((phase # "Superseded") \/ (superseded_by_revision # None))
delivery_failed_records_failure_class == ((phase # "DeliveryFailed") \/ (failure_class # None))
misfire_deadline_not_before_due == (misfire_deadline_utc_ms >= due_at_utc_ms)

CiStateConstraint == /\ model_step_count <= 6
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []live_claim_requires_owner
THEOREM Spec => []superseded_records_revision
THEOREM Spec => []delivery_failed_records_failure_class
THEOREM Spec => []misfire_deadline_not_before_due

=============================================================================
