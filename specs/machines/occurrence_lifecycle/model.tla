---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OccurrenceLifecycleMachine.

CONSTANTS BooleanValues, ClaimOwnerValues, ClaimTokenValues, ClaimedDispatchDispositionValues, ClaimedDispatchSchedulePhaseValues, CompletionSupersessionDispositionValues, CorrelationIdValues, DeliveryCompletionFailureReasonValues, DeliveryFailureReasonValues, DeliveryReceiptStageValues, LateCompletionResolutionClassValues, MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, OccurrenceLifecycleInputVariantValues, OccurrenceLifecycleStateValues, OccurrenceTargetProbeOutcomeValues, OccurrenceTransitionFailureClassKindValues, OccurrenceTransitionFailureRefusalKindValues, OverlapPolicyValues, RuntimeCompletionOutcomeValues, RuntimeOutcomeKeyValues, ScheduleIdValues, SessionIdValues, StringValues, TargetBindingIdValues, TriggerKeyValues

None == [tag |-> "none", value |-> "none"]
Some(v) == [tag |-> "some", value |-> v]

OptionClaimOwnerValues == {None} \cup {Some(x) : x \in ClaimOwnerValues}
OptionClaimTokenValues == {None} \cup {Some(x) : x \in ClaimTokenValues}
OptionCorrelationIdValues == {None} \cup {Some(x) : x \in CorrelationIdValues}
OptionDeliveryReceiptStageValues == {None} \cup {Some(x) : x \in DeliveryReceiptStageValues}
OptionLateCompletionResolutionClassValues == {None} \cup {Some(x) : x \in LateCompletionResolutionClassValues}
OptionOccurrenceFailureClassValues == {None} \cup {Some(x) : x \in OccurrenceFailureClassValues}
OptionRuntimeOutcomeKeyValues == {None} \cup {Some(x) : x \in RuntimeOutcomeKeyValues}
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

VARIABLES phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals

vars == << phase, model_step_count, occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>

is_live_claim_phase(arg_phase) == (IF (arg_phase = "Claimed") THEN TRUE ELSE (IF (arg_phase = "Dispatching") THEN TRUE ELSE (arg_phase = "AwaitingCompletion")))

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
    /\ late_completion_recorded_at_utc_ms = None
    /\ late_completion_resolution = None
    /\ late_completion_detail = None
    /\ stale_completion_arrivals = 0

TerminalStutter ==
    /\ phase = "Completed" \/ phase = "Skipped" \/ phase = "Misfired" \/ phase = "Superseded" \/ phase = "DeliveryFailed"
    /\ UNCHANGED vars

ClassifyTransitionFailurePlanRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailurePlanRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimedDispatchDispositionRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureCompletionSupersessionRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureClaimRejectedPendingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "Claim") /\ (refusal_kind = "GuardRejected"))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotClaimedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotDispatchingDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "AwaitCompletion") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (trigger = "ReleaseLeaseForPausedSchedule")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyTransitionFailureStaleCompletionArrivalRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyStaleCompletionArrivalObservedPending(trigger) ==
    /\ phase = "Pending"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedClaimed(trigger) ==
    /\ phase = "Claimed"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedDispatching(trigger) ==
    /\ phase = "Dispatching"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedAwaitingCompletion(trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedCompleted(trigger) ==
    /\ phase = "Completed"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedSkipped(trigger) ==
    /\ phase = "Skipped"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedMisfired(trigger) ==
    /\ phase = "Misfired"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedSuperseded(trigger) ==
    /\ phase = "Superseded"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


ClassifyStaleCompletionArrivalObservedDeliveryFailed(trigger) ==
    /\ phase = "DeliveryFailed"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>


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
    /\ late_completion_recorded_at_utc_ms' = None
    /\ late_completion_resolution' = None
    /\ late_completion_detail' = None
    /\ stale_completion_arrivals' = 0


ClassifyDuePendingFuture(now_utc_ms) ==
    /\ phase = "Pending"
    /\ (now_utc_ms < due_at_utc_ms)
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDuePendingMisfire(now_utc_ms) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= now_utc_ms) /\ (misfire_deadline_utc_ms < now_utc_ms))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDuePendingClaimEligible(now_utc_ms) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= now_utc_ms) /\ (now_utc_ms <= misfire_deadline_utc_ms))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueClaimedLeaseExpired(now_utc_ms) ==
    /\ phase = "Claimed"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueDispatchingLeaseExpired(now_utc_ms) ==
    /\ phase = "Dispatching"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueAwaitingCompletionLeaseExpired(now_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueClaimedLeaseCurrent(now_utc_ms) ==
    /\ phase = "Claimed"
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueDispatchingLeaseCurrent(now_utc_ms) ==
    /\ phase = "Dispatching"
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueAwaitingCompletionLeaseCurrent(now_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueCompletedNoAction(now_utc_ms) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueSkippedNoAction(now_utc_ms) ==
    /\ phase = "Skipped"
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueMisfiredNoAction(now_utc_ms) ==
    /\ phase = "Misfired"
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueSupersededNoAction(now_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyDueDeliveryFailedNoAction(now_utc_ms) ==
    /\ phase = "DeliveryFailed"
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityTerminalCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityTerminalSkipped ==
    /\ phase = "Skipped"
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityTerminalMisfired ==
    /\ phase = "Misfired"
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityTerminalSuperseded ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityTerminalDeliveryFailed ==
    /\ phase = "DeliveryFailed"
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityLivePending ==
    /\ phase = "Pending"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityLiveClaimed ==
    /\ phase = "Claimed"
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityLiveDispatching ==
    /\ phase = "Dispatching"
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyOccurrenceTerminalityLiveAwaitingCompletion ==
    /\ phase = "AwaitingCompletion"
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyClaimedDispatchDispositionFutureRevision(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ (current_schedule_revision < schedule_revision)
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyClaimedDispatchDispositionFrozen(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((current_schedule_revision >= schedule_revision) /\ (schedule_phase = "Paused"))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyClaimedDispatchDispositionSupersedeDeleted(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((current_schedule_revision >= schedule_revision) /\ (schedule_phase = "Deleted"))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyClaimedDispatchDispositionSupersedeStale(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((schedule_phase = "Active") /\ (schedule_revision < current_schedule_revision))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyClaimedDispatchDispositionReady(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((schedule_phase = "Active") /\ (schedule_revision = current_schedule_revision))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyCompletionSupersessionDeleted(schedule_phase, current_schedule_revision) ==
    /\ phase = "AwaitingCompletion"
    /\ (schedule_phase = "Deleted")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyCompletionSupersessionStale(schedule_phase, current_schedule_revision) ==
    /\ phase = "AwaitingCompletion"
    /\ ((schedule_phase # "Deleted") /\ (schedule_revision < current_schedule_revision))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyCompletionSupersessionProceed(schedule_phase, current_schedule_revision) ==
    /\ phase = "AwaitingCompletion"
    /\ ((schedule_phase # "Deleted") /\ (schedule_revision >= current_schedule_revision))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ClassifyCompletionSupersessionAlreadySuperseded(schedule_phase, current_schedule_revision) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ phase = "Pending"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ target_materialized_session_id' = arg_target_materialized_session_id
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ phase = "Claimed"
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ target_materialized_session_id' = arg_target_materialized_session_id
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


AwaitCompletionFromDispatching(at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ dispatched_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


AwaitCompletionAfterSupersession(at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


CompleteFromDispatchingOrAwaiting(at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Completed")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


RuntimeCompletionRuntimeRejected(outcome, detail, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (IF (outcome = "CallbackPending") THEN TRUE ELSE (IF (outcome = "Cancelled") THEN TRUE ELSE (outcome = "Abandoned")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("RuntimeRejected")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("RuntimeRejected")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryCompletionFailureTransportError(reason, detail, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "CompletionFutureFailed")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("TransportError")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TransportError")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryCompletionFailureInternalError(reason, detail, at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (IF (reason = "RuntimeCompletionChannelClosed") THEN TRUE ELSE (IF (reason = "RuntimeCompletionAuthorityUnavailable") THEN TRUE ELSE (reason = "RuntimeCompletionHandleMissing")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("InternalError")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("InternalError")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryFailureTargetMaterializationFailed(reason, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "TargetMaterializationFailed")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("TargetMaterializationFailed")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TargetMaterializationFailed")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryFailureTargetMissing(reason, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "TargetMissing")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("TargetMissing")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TargetMissing")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryFailureTargetBusy(reason, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "TargetBusy")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("TargetBusy")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TargetBusy")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryFailureRuntimeRejected(reason, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "RuntimeRejected")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("RuntimeRejected")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("RuntimeRejected")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryFailureMobRejected(reason, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "MobRejected")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("MobRejected")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("MobRejected")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryFailureTransportError(reason, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "TransportError")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("TransportError")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TransportError")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DeliveryFailureInternalError(reason, detail, at_utc_ms) ==
    /\ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ (reason = "InternalError")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DeliveryFailed")
    /\ receipt_failure_class' = Some("InternalError")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("InternalError")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


TargetProbeReadyClaimed(outcome, detail, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ (outcome = "Ready")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


TargetProbeBusyAllowedByPolicy(outcome, detail, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ (outcome = "Busy")
    /\ (overlap_policy = "AllowConcurrent")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


TargetProbeBusySkipByPolicy(outcome, detail, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ (outcome = "Busy")
    /\ (overlap_policy = "SkipIfRunning")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Skipped")
    /\ receipt_failure_class' = Some("TargetBusy")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TargetBusy")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_at_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


TargetProbeMissingSkipByPolicy(outcome, detail, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ (outcome = "Missing")
    /\ (missing_target_policy = "Skip")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Skipped")
    /\ receipt_failure_class' = Some("TargetMissing")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TargetMissing")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_at_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


TargetProbeMissingMisfireByPolicy(outcome, detail, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ (outcome = "Missing")
    /\ (missing_target_policy = "MarkMisfired")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Misfired")
    /\ receipt_failure_class' = Some("TargetMissing")
    /\ receipt_detail' = detail
    /\ failure_class' = Some("TargetMissing")
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_at_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


DueMisfirePending(detail, at_utc_ms) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= at_utc_ms) /\ (misfire_deadline_utc_ms < at_utc_ms))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ claimed_by' = None
    /\ lease_expires_at_utc_ms' = None
    /\ claim_token' = None
    /\ delivery_correlation_id' = None
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Misfired")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = detail
    /\ failure_class' = None
    /\ failure_detail' = detail
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_at_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


SupersedeAlreadySuperseded(arg_superseded_by_revision, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


LateCompleteAfterSupersession(at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("DeliveryCompleted")
    /\ late_completion_detail' = None
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>


LateRuntimeCompletionCompletedAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (outcome = "Completed")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeCompleted")
    /\ late_completion_detail' = detail
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>


LateRuntimeCompletionRejectedAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (IF (outcome = "CallbackPending") THEN TRUE ELSE (IF (outcome = "Cancelled") THEN TRUE ELSE (outcome = "Abandoned")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeRejected")
    /\ late_completion_detail' = detail
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>


LateRuntimeCompletionTransportErrorAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (outcome = "RuntimeTerminated")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeTransportError")
    /\ late_completion_detail' = detail
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>


LateRuntimeCompletionInternalErrorAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (outcome = "FinalizationFailed")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeInternalError")
    /\ late_completion_detail' = detail
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>


LateDeliveryCompletionFailureAfterSupersession(reason, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("DeliveryCompletionFailed")
    /\ late_completion_detail' = detail
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>


LateDeliveryFailureAfterSupersession(reason, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("DeliveryFailed")
    /\ late_completion_detail' = detail
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


ReleaseLeaseForPausedScheduleAfterSupersession(at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>


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
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedDeliveryFailed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedDeliveryFailed(refusal_kind, trigger)
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
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedPending(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedClaimed(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedDispatching(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedCompleted(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedSkipped(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedMisfired(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedSuperseded(refusal_kind, trigger)
    \/ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedDeliveryFailed(refusal_kind, trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedPending(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedClaimed(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedDispatching(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedAwaitingCompletion(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedCompleted(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedSkipped(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedMisfired(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedSuperseded(trigger)
    \/ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedDeliveryFailed(trigger)
    \/ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in TriggerKeyValues : \E arg_target_binding_key \in TargetBindingIdValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms)
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
    \/ ClassifyOccurrenceTerminalityTerminalCompleted
    \/ ClassifyOccurrenceTerminalityTerminalSkipped
    \/ ClassifyOccurrenceTerminalityTerminalMisfired
    \/ ClassifyOccurrenceTerminalityTerminalSuperseded
    \/ ClassifyOccurrenceTerminalityTerminalDeliveryFailed
    \/ ClassifyOccurrenceTerminalityLivePending
    \/ ClassifyOccurrenceTerminalityLiveClaimed
    \/ ClassifyOccurrenceTerminalityLiveDispatching
    \/ ClassifyOccurrenceTerminalityLiveAwaitingCompletion
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionFutureRevision(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionFrozen(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionSupersedeDeleted(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionSupersedeStale(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionReady(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionDeleted(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionStale(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionProceed(schedule_phase, current_schedule_revision)
    \/ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionAlreadySuperseded(schedule_phase, current_schedule_revision)
    \/ \E arg_target_binding_key \in TargetBindingIdValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id)
    \/ \E arg_target_binding_key \in TargetBindingIdValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptPending(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptClaimed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptDispatching(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptAwaitingCompletion(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptCompleted(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptSkipped(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptMisfired(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptSuperseded(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptDeliveryFailed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ \E owner_id \in ClaimOwnerValues : \E at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : ClaimPending(owner_id, at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)
    \/ \E correlation_id \in OptionCorrelationIdValues : \E at_utc_ms \in 0..2 : DispatchStartedFromClaimed(correlation_id, at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : AwaitCompletionFromDispatching(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : AwaitCompletionAfterSupersession(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : CompleteFromDispatchingOrAwaiting(at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionCompleted(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionRuntimeRejected(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionTransportError(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionInternalError(outcome, detail, at_utc_ms)
    \/ \E reason \in DeliveryCompletionFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryCompletionFailureTransportError(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryCompletionFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryCompletionFailureInternalError(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTargetMaterializationFailed(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTargetMissing(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTargetBusy(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureRuntimeRejected(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureMobRejected(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTransportError(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureInternalError(reason, detail, at_utc_ms)
    \/ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeReadyClaimed(outcome, detail, at_utc_ms)
    \/ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeBusyAllowedByPolicy(outcome, detail, at_utc_ms)
    \/ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeBusySkipByPolicy(outcome, detail, at_utc_ms)
    \/ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeMissingSkipByPolicy(outcome, detail, at_utc_ms)
    \/ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeMissingMisfireByPolicy(outcome, detail, at_utc_ms)
    \/ \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DueMisfirePending(detail, at_utc_ms)
    \/ \E arg_superseded_by_revision \in 0..2 : \E at_utc_ms \in 0..2 : SupersedePendingOrLive(arg_superseded_by_revision, at_utc_ms)
    \/ \E arg_superseded_by_revision \in 0..2 : \E at_utc_ms \in 0..2 : SupersedeAlreadySuperseded(arg_superseded_by_revision, at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LateCompleteAfterSupersession(at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionCompletedAfterSupersession(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionRejectedAfterSupersession(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionTransportErrorAfterSupersession(outcome, detail, at_utc_ms)
    \/ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionInternalErrorAfterSupersession(outcome, detail, at_utc_ms)
    \/ \E reason \in DeliveryCompletionFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateDeliveryCompletionFailureAfterSupersession(reason, detail, at_utc_ms)
    \/ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateDeliveryFailureAfterSupersession(reason, detail, at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromClaimed(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromDispatching(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : LeaseExpiredFromAwaitingCompletion(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromClaimed(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromDispatching(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromAwaitingCompletion(at_utc_ms)
    \/ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleAfterSupersession(at_utc_ms)
    \/ TerminalStutter

live_claim_requires_owner == (IF ~(is_live_claim_phase(phase)) THEN TRUE ELSE (claimed_by # None))
superseded_records_revision == (IF (phase # "Superseded") THEN TRUE ELSE (superseded_by_revision # None))
delivery_failed_records_failure_class == (IF (phase # "DeliveryFailed") THEN TRUE ELSE (failure_class # None))
misfire_deadline_not_before_due == (misfire_deadline_utc_ms >= due_at_utc_ms)
late_completion_only_after_supersession == (IF (late_completion_resolution = None) THEN TRUE ELSE (phase = "Superseded"))
late_completion_resolution_requires_timestamp == (IF (late_completion_resolution = None) THEN TRUE ELSE (late_completion_recorded_at_utc_ms # None))
late_completion_timestamp_requires_resolution == (IF (late_completion_recorded_at_utc_ms = None) THEN TRUE ELSE (late_completion_resolution # None))
late_completion_detail_requires_resolution == (IF (late_completion_detail = None) THEN TRUE ELSE (late_completion_resolution # None))

CiStateConstraint == /\ model_step_count <= 3
DeepStateConstraint == /\ model_step_count <= 8

Spec == Init /\ [][Next]_vars

THEOREM Spec => []live_claim_requires_owner
THEOREM Spec => []superseded_records_revision
THEOREM Spec => []delivery_failed_records_failure_class
THEOREM Spec => []misfire_deadline_not_before_due
THEOREM Spec => []late_completion_only_after_supersession
THEOREM Spec => []late_completion_resolution_requires_timestamp
THEOREM Spec => []late_completion_timestamp_requires_resolution
THEOREM Spec => []late_completion_detail_requires_resolution

=============================================================================
