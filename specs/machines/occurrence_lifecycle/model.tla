---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OccurrenceLifecycleMachine.

CONSTANTS BooleanValues, ClaimOwnerValues, ClaimTokenValues, ClaimedDispatchDispositionValues, ClaimedDispatchSchedulePhaseValues, CompletionSupersessionDispositionValues, CorrelationIdValues, DeliveryAdmissionOutcomeValues, DeliveryCompletionFailureReasonValues, DeliveryFailureReasonValues, DeliveryReceiptStageValues, LateCompletionResolutionClassValues, MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, OccurrenceLifecycleInputVariantValues, OccurrenceLifecycleStateValues, OccurrenceTargetProbeOutcomeValues, OccurrenceTransitionFailureClassKindValues, OccurrenceTransitionFailureRefusalKindValues, OverlapPolicyValues, RuntimeCompletionOutcomeValues, RuntimeOutcomeKeyValues, ScheduleIdValues, SessionIdValues, StringValues, TargetBindingIdValues, TriggerKeyValues

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

\* Named UNCHANGED frames. One definition per distinct frame; every action
\* that leaves those variables unchanged references the definition by name.
UnchangedFrame_0b1d331647e227f9 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_1f2faaf9e88439a0 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_at_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_3970f5df13f0db16 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_4ce826df73aaaea4 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_6cd77dd931d13dea == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, stale_completion_arrivals >>
UnchangedFrame_792f0c8d04e6b907 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_79b9bec285f71b9e == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail >>
UnchangedFrame_8de17078a40e0109 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_95e30a42bf11dec6 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_9adccbdc52b0d96a == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, attempt_count, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_baba76eb08ae0995 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, dispatched_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_d29ebca6d675ee12 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_dac7ac5b0d5b99b3 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_e096a8be19cf8a1c == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>
UnchangedFrame_f04d76cc9194d8c9 == UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision, late_completion_recorded_at_utc_ms, late_completion_resolution, late_completion_detail, stale_completion_arrivals >>

ClassifyTransitionFailurePlanRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailurePlanRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "PlanOccurrence") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "SyncTargetSnapshot") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "RecordReceipt") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyDue") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimedDispatchDispositionRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyClaimedDispatchDisposition") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureCompletionSupersessionRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyCompletionSupersession") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureClaimRejectedPendingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "Claim") /\ (refusal_kind = "GuardRejected"))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "Claim") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotClaimedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "DispatchStarted") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotDispatchingDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((IF (trigger = "DispatchAccepted") THEN TRUE ELSE (trigger = "AwaitCompletion")) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((IF (trigger = "LeaseExpired") THEN TRUE ELSE (IF (trigger = "ReleaseLeaseForPausedSchedule") THEN TRUE ELSE (trigger = "RenewLease"))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (IF (trigger = "ResolveTargetProbe") THEN TRUE ELSE (IF (trigger = "ResolveDueMisfire") THEN TRUE ELSE (trigger = "Supersede"))))))) /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedPending(refusal_kind, trigger) ==
    /\ phase = "Pending"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedClaimed(refusal_kind, trigger) ==
    /\ phase = "Claimed"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedDispatching(refusal_kind, trigger) ==
    /\ phase = "Dispatching"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedAwaitingCompletion(refusal_kind, trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedCompleted(refusal_kind, trigger) ==
    /\ phase = "Completed"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedSkipped(refusal_kind, trigger) ==
    /\ phase = "Skipped"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedMisfired(refusal_kind, trigger) ==
    /\ phase = "Misfired"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedSuperseded(refusal_kind, trigger) ==
    /\ phase = "Superseded"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyTransitionFailureStaleCompletionArrivalRejectedDeliveryFailed(refusal_kind, trigger) ==
    /\ phase = "DeliveryFailed"
    /\ ((trigger = "ClassifyStaleCompletionArrival") /\ (IF (refusal_kind = "GuardRejected") THEN TRUE ELSE (refusal_kind = "NoMatchingTransition")))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyStaleCompletionArrivalObservedPending(trigger) ==
    /\ phase = "Pending"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedClaimed(trigger) ==
    /\ phase = "Claimed"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedDispatching(trigger) ==
    /\ phase = "Dispatching"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedAwaitingCompletion(trigger) ==
    /\ phase = "AwaitingCompletion"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedCompleted(trigger) ==
    /\ phase = "Completed"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedSkipped(trigger) ==
    /\ phase = "Skipped"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedMisfired(trigger) ==
    /\ phase = "Misfired"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedSuperseded(trigger) ==
    /\ phase = "Superseded"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


ClassifyStaleCompletionArrivalObservedDeliveryFailed(trigger) ==
    /\ phase = "DeliveryFailed"
    /\ (IF (trigger = "Complete") THEN TRUE ELSE (IF (trigger = "ResolveRuntimeCompletion") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryCompletionFailure") THEN TRUE ELSE (IF (trigger = "ResolveDeliveryFailure") THEN TRUE ELSE (trigger = "Supersede")))))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ stale_completion_arrivals' = (stale_completion_arrivals) + 1
    /\ UnchangedFrame_79b9bec285f71b9e


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
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDuePendingMisfire(now_utc_ms) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= now_utc_ms) /\ (misfire_deadline_utc_ms < now_utc_ms))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDuePendingClaimEligible(now_utc_ms) ==
    /\ phase = "Pending"
    /\ ((due_at_utc_ms <= now_utc_ms) /\ (now_utc_ms <= misfire_deadline_utc_ms))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueClaimedLeaseExpired(now_utc_ms) ==
    /\ phase = "Claimed"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueDispatchingLeaseExpired(now_utc_ms) ==
    /\ phase = "Dispatching"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueAwaitingCompletionLeaseExpired(now_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ ((lease_expires_at_utc_ms # None) /\ ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= now_utc_ms))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueClaimedLeaseCurrent(now_utc_ms) ==
    /\ phase = "Claimed"
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueDispatchingLeaseCurrent(now_utc_ms) ==
    /\ phase = "Dispatching"
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueAwaitingCompletionLeaseCurrent(now_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE (now_utc_ms < (IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None)))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueCompletedNoAction(now_utc_ms) ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueSkippedNoAction(now_utc_ms) ==
    /\ phase = "Skipped"
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueMisfiredNoAction(now_utc_ms) ==
    /\ phase = "Misfired"
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueSupersededNoAction(now_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyDueDeliveryFailedNoAction(now_utc_ms) ==
    /\ phase = "DeliveryFailed"
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityTerminalCompleted ==
    /\ phase = "Completed"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityTerminalSkipped ==
    /\ phase = "Skipped"
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityTerminalMisfired ==
    /\ phase = "Misfired"
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityTerminalSuperseded ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityTerminalDeliveryFailed ==
    /\ phase = "DeliveryFailed"
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityLivePending ==
    /\ phase = "Pending"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityLiveClaimed ==
    /\ phase = "Claimed"
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityLiveDispatching ==
    /\ phase = "Dispatching"
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyOccurrenceTerminalityLiveAwaitingCompletion ==
    /\ phase = "AwaitingCompletion"
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyClaimedDispatchDispositionFutureRevision(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ (current_schedule_revision < schedule_revision)
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyClaimedDispatchDispositionFrozen(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((current_schedule_revision >= schedule_revision) /\ (schedule_phase = "Paused"))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyClaimedDispatchDispositionSupersedeDeleted(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((current_schedule_revision >= schedule_revision) /\ (schedule_phase = "Deleted"))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyClaimedDispatchDispositionSupersedeStale(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((schedule_phase = "Active") /\ (schedule_revision < current_schedule_revision))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyClaimedDispatchDispositionReady(schedule_phase, current_schedule_revision) ==
    /\ phase = "Claimed"
    /\ ((schedule_phase = "Active") /\ (schedule_revision = current_schedule_revision))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyCompletionSupersessionDeleted(schedule_phase, current_schedule_revision) ==
    /\ phase = "AwaitingCompletion"
    /\ (schedule_phase = "Deleted")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyCompletionSupersessionStale(schedule_phase, current_schedule_revision) ==
    /\ phase = "AwaitingCompletion"
    /\ ((schedule_phase # "Deleted") /\ (schedule_revision < current_schedule_revision))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyCompletionSupersessionProceed(schedule_phase, current_schedule_revision) ==
    /\ phase = "AwaitingCompletion"
    /\ ((schedule_phase # "Deleted") /\ (schedule_revision >= current_schedule_revision))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


ClassifyCompletionSupersessionAlreadySuperseded(schedule_phase, current_schedule_revision) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ phase = "Pending"
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ target_materialized_session_id' = arg_target_materialized_session_id
    /\ UnchangedFrame_e096a8be19cf8a1c


SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id) ==
    /\ phase = "Claimed"
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ target_binding_key' = arg_target_binding_key
    /\ target_materialized_session_id' = arg_target_materialized_session_id
    /\ UnchangedFrame_e096a8be19cf8a1c


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_8de17078a40e0109


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
    /\ UnchangedFrame_0b1d331647e227f9


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
    /\ UnchangedFrame_792f0c8d04e6b907


DispatchAcceptedFromDispatching(admission_outcome, at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ (delivery_correlation_id # None)
    /\ (admission_outcome = "Accepted")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("DispatchAccepted")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ UnchangedFrame_f04d76cc9194d8c9


DispatchDeduplicatedFromDispatching(admission_outcome, at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ (delivery_correlation_id # None)
    /\ (admission_outcome = "Deduplicated")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Completed")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_95e30a42bf11dec6


AwaitCompletionFromDispatching(at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ dispatched_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_d29ebca6d675ee12


AwaitCompletionAfterSupersession(at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


CompleteFromDispatchingOrAwaiting(at_utc_ms) ==
    /\ phase = "Dispatching" \/ phase = "AwaitingCompletion"
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ receipt_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ receipt_stage' = Some("Completed")
    /\ receipt_failure_class' = None
    /\ receipt_detail' = None
    /\ completed_at_utc_ms' = Some(at_utc_ms)
    /\ UnchangedFrame_95e30a42bf11dec6


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
    /\ UnchangedFrame_95e30a42bf11dec6


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


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
    /\ UnchangedFrame_baba76eb08ae0995


TargetProbeReadyClaimed(outcome, detail, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ (outcome = "Ready")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


TargetProbeBusyAllowedByPolicy(outcome, detail, at_utc_ms) ==
    /\ phase = "Claimed"
    /\ (outcome = "Busy")
    /\ (overlap_policy = "AllowConcurrent")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


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
    /\ UnchangedFrame_1f2faaf9e88439a0


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
    /\ UnchangedFrame_1f2faaf9e88439a0


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
    /\ UnchangedFrame_1f2faaf9e88439a0


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
    /\ UnchangedFrame_1f2faaf9e88439a0


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
    /\ UnchangedFrame_9adccbdc52b0d96a


SupersedeAlreadySuperseded(arg_superseded_by_revision, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


LateCompleteAfterSupersession(at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("DeliveryCompleted")
    /\ late_completion_detail' = None
    /\ UnchangedFrame_6cd77dd931d13dea


LateRuntimeCompletionCompletedAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (outcome = "Completed")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeCompleted")
    /\ late_completion_detail' = detail
    /\ UnchangedFrame_6cd77dd931d13dea


LateRuntimeCompletionRejectedAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (IF (outcome = "CallbackPending") THEN TRUE ELSE (IF (outcome = "Cancelled") THEN TRUE ELSE (outcome = "Abandoned")))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeRejected")
    /\ late_completion_detail' = detail
    /\ UnchangedFrame_6cd77dd931d13dea


LateRuntimeCompletionTransportErrorAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (outcome = "RuntimeTerminated")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeTransportError")
    /\ late_completion_detail' = detail
    /\ UnchangedFrame_6cd77dd931d13dea


LateRuntimeCompletionInternalErrorAfterSupersession(outcome, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ (outcome = "FinalizationFailed")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("RuntimeInternalError")
    /\ late_completion_detail' = detail
    /\ UnchangedFrame_6cd77dd931d13dea


LateDeliveryCompletionFailureAfterSupersession(reason, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("DeliveryCompletionFailed")
    /\ late_completion_detail' = detail
    /\ UnchangedFrame_6cd77dd931d13dea


LateDeliveryFailureAfterSupersession(reason, detail, at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ late_completion_recorded_at_utc_ms' = Some(at_utc_ms)
    /\ late_completion_resolution' = Some("DeliveryFailed")
    /\ late_completion_detail' = detail
    /\ UnchangedFrame_6cd77dd931d13dea


RenewLeaseFromDispatching(arg_claim_token, arg_lease_expires_at_utc_ms, at_utc_ms) ==
    /\ phase = "Dispatching"
    /\ (claim_token = Some(arg_claim_token))
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= arg_lease_expires_at_utc_ms))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ lease_expires_at_utc_ms' = Some(arg_lease_expires_at_utc_ms)
    /\ UnchangedFrame_4ce826df73aaaea4


RenewLeaseFromAwaitingCompletion(arg_claim_token, arg_lease_expires_at_utc_ms, at_utc_ms) ==
    /\ phase = "AwaitingCompletion"
    /\ (claim_token = Some(arg_claim_token))
    /\ (IF (lease_expires_at_utc_ms = None) THEN TRUE ELSE ((IF "value" \in DOMAIN lease_expires_at_utc_ms THEN lease_expires_at_utc_ms["value"] ELSE None) <= arg_lease_expires_at_utc_ms))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ lease_expires_at_utc_ms' = Some(arg_lease_expires_at_utc_ms)
    /\ UnchangedFrame_4ce826df73aaaea4


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
    /\ UnchangedFrame_3970f5df13f0db16


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
    /\ UnchangedFrame_3970f5df13f0db16


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
    /\ UnchangedFrame_3970f5df13f0db16


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
    /\ UnchangedFrame_3970f5df13f0db16


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
    /\ UnchangedFrame_3970f5df13f0db16


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
    /\ UnchangedFrame_3970f5df13f0db16


ReleaseLeaseForPausedScheduleAfterSupersession(at_utc_ms) ==
    /\ phase = "Superseded"
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UnchangedFrame_dac7ac5b0d5b99b3


Next ==
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailurePlanRejectedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimedDispatchDispositionRejectedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureCompletionSupersessionRejectedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureClaimRejectedPendingPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotClaimedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotDispatchingDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedPending(refusal_kind, trigger)
    \/ (phase = "Claimed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedClaimed(refusal_kind, trigger)
    \/ (phase = "Dispatching") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedDispatching(refusal_kind, trigger)
    \/ (phase = "AwaitingCompletion") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedAwaitingCompletion(refusal_kind, trigger)
    \/ (phase = "Completed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedCompleted(refusal_kind, trigger)
    \/ (phase = "Skipped") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedSkipped(refusal_kind, trigger)
    \/ (phase = "Misfired") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedMisfired(refusal_kind, trigger)
    \/ (phase = "Superseded") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedSuperseded(refusal_kind, trigger)
    \/ (phase = "DeliveryFailed") /\ \E refusal_kind \in OccurrenceTransitionFailureRefusalKindValues : \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyTransitionFailureStaleCompletionArrivalRejectedDeliveryFailed(refusal_kind, trigger)
    \/ (phase = "Pending") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedPending(trigger)
    \/ (phase = "Claimed") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedClaimed(trigger)
    \/ (phase = "Dispatching") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedDispatching(trigger)
    \/ (phase = "AwaitingCompletion") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedAwaitingCompletion(trigger)
    \/ (phase = "Completed") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedCompleted(trigger)
    \/ (phase = "Skipped") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedSkipped(trigger)
    \/ (phase = "Misfired") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedMisfired(trigger)
    \/ (phase = "Superseded") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedSuperseded(trigger)
    \/ (phase = "DeliveryFailed") /\ \E trigger \in OccurrenceLifecycleInputVariantValues : ClassifyStaleCompletionArrivalObservedDeliveryFailed(trigger)
    \/ (phase = "Pending") /\ \E arg_occurrence_id \in OccurrenceIdValues : \E arg_schedule_id \in ScheduleIdValues : \E arg_schedule_revision \in 0..2 : \E arg_occurrence_ordinal \in 0..2 : \E arg_trigger_key \in TriggerKeyValues : \E arg_target_binding_key \in TargetBindingIdValues : \E arg_misfire_policy \in MisfirePolicyValues : \E arg_misfire_policy_key \in StringValues : \E arg_overlap_policy \in OverlapPolicyValues : \E arg_overlap_policy_key \in StringValues : \E arg_missing_target_policy \in MissingTargetPolicyValues : \E arg_missing_target_policy_key \in StringValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : \E arg_due_at_utc_ms \in 0..2 : \E arg_misfire_deadline_utc_ms \in 0..2 : PlanOccurrenceFromPending(arg_occurrence_id, arg_schedule_id, arg_schedule_revision, arg_occurrence_ordinal, arg_trigger_key, arg_target_binding_key, arg_misfire_policy, arg_misfire_policy_key, arg_overlap_policy, arg_overlap_policy_key, arg_missing_target_policy, arg_missing_target_policy_key, arg_target_materialized_session_id, arg_due_at_utc_ms, arg_misfire_deadline_utc_ms)
    \/ (phase = "Pending") /\ \E now_utc_ms \in 0..2 : ClassifyDuePendingFuture(now_utc_ms)
    \/ (phase = "Pending") /\ \E now_utc_ms \in 0..2 : ClassifyDuePendingMisfire(now_utc_ms)
    \/ (phase = "Pending") /\ \E now_utc_ms \in 0..2 : ClassifyDuePendingClaimEligible(now_utc_ms)
    \/ (phase = "Claimed") /\ \E now_utc_ms \in 0..2 : ClassifyDueClaimedLeaseExpired(now_utc_ms)
    \/ (phase = "Dispatching") /\ \E now_utc_ms \in 0..2 : ClassifyDueDispatchingLeaseExpired(now_utc_ms)
    \/ (phase = "AwaitingCompletion") /\ \E now_utc_ms \in 0..2 : ClassifyDueAwaitingCompletionLeaseExpired(now_utc_ms)
    \/ (phase = "Claimed") /\ \E now_utc_ms \in 0..2 : ClassifyDueClaimedLeaseCurrent(now_utc_ms)
    \/ (phase = "Dispatching") /\ \E now_utc_ms \in 0..2 : ClassifyDueDispatchingLeaseCurrent(now_utc_ms)
    \/ (phase = "AwaitingCompletion") /\ \E now_utc_ms \in 0..2 : ClassifyDueAwaitingCompletionLeaseCurrent(now_utc_ms)
    \/ (phase = "Completed") /\ \E now_utc_ms \in 0..2 : ClassifyDueCompletedNoAction(now_utc_ms)
    \/ (phase = "Skipped") /\ \E now_utc_ms \in 0..2 : ClassifyDueSkippedNoAction(now_utc_ms)
    \/ (phase = "Misfired") /\ \E now_utc_ms \in 0..2 : ClassifyDueMisfiredNoAction(now_utc_ms)
    \/ (phase = "Superseded") /\ \E now_utc_ms \in 0..2 : ClassifyDueSupersededNoAction(now_utc_ms)
    \/ (phase = "DeliveryFailed") /\ \E now_utc_ms \in 0..2 : ClassifyDueDeliveryFailedNoAction(now_utc_ms)
    \/ ClassifyOccurrenceTerminalityTerminalCompleted
    \/ ClassifyOccurrenceTerminalityTerminalSkipped
    \/ ClassifyOccurrenceTerminalityTerminalMisfired
    \/ ClassifyOccurrenceTerminalityTerminalSuperseded
    \/ ClassifyOccurrenceTerminalityTerminalDeliveryFailed
    \/ ClassifyOccurrenceTerminalityLivePending
    \/ ClassifyOccurrenceTerminalityLiveClaimed
    \/ ClassifyOccurrenceTerminalityLiveDispatching
    \/ ClassifyOccurrenceTerminalityLiveAwaitingCompletion
    \/ (phase = "Claimed") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionFutureRevision(schedule_phase, current_schedule_revision)
    \/ (phase = "Claimed") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionFrozen(schedule_phase, current_schedule_revision)
    \/ (phase = "Claimed") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionSupersedeDeleted(schedule_phase, current_schedule_revision)
    \/ (phase = "Claimed") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionSupersedeStale(schedule_phase, current_schedule_revision)
    \/ (phase = "Claimed") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyClaimedDispatchDispositionReady(schedule_phase, current_schedule_revision)
    \/ (phase = "AwaitingCompletion") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionDeleted(schedule_phase, current_schedule_revision)
    \/ (phase = "AwaitingCompletion") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionStale(schedule_phase, current_schedule_revision)
    \/ (phase = "AwaitingCompletion") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionProceed(schedule_phase, current_schedule_revision)
    \/ (phase = "Superseded") /\ \E schedule_phase \in ClaimedDispatchSchedulePhaseValues : \E current_schedule_revision \in 0..2 : ClassifyCompletionSupersessionAlreadySuperseded(schedule_phase, current_schedule_revision)
    \/ (phase = "Pending") /\ \E arg_target_binding_key \in TargetBindingIdValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : SyncTargetSnapshotPending(arg_target_binding_key, arg_target_materialized_session_id)
    \/ (phase = "Claimed") /\ \E arg_target_binding_key \in TargetBindingIdValues : \E arg_target_materialized_session_id \in OptionSessionIdValues : SyncTargetSnapshotClaimed(arg_target_binding_key, arg_target_materialized_session_id)
    \/ (phase = "Pending") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptPending(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "Claimed") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptClaimed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "Dispatching") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptDispatching(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "AwaitingCompletion") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptAwaitingCompletion(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "Completed") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptCompleted(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "Skipped") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptSkipped(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "Misfired") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptMisfired(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "Superseded") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptSuperseded(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "DeliveryFailed") /\ \E correlation_id \in OptionCorrelationIdValues : \E detail \in OptionStringValues : \E materialized_session_id \in OptionSessionIdValues : \E arg_runtime_outcome_key \in OptionRuntimeOutcomeKeyValues : RecordReceiptDeliveryFailed(correlation_id, detail, materialized_session_id, arg_runtime_outcome_key)
    \/ (phase = "Pending") /\ \E owner_id \in ClaimOwnerValues : \E at_utc_ms \in 0..2 : \E arg_lease_expires_at_utc_ms \in 0..2 : \E arg_claim_token \in ClaimTokenValues : ClaimPending(owner_id, at_utc_ms, arg_lease_expires_at_utc_ms, arg_claim_token)
    \/ (phase = "Claimed") /\ \E correlation_id \in OptionCorrelationIdValues : \E at_utc_ms \in 0..2 : DispatchStartedFromClaimed(correlation_id, at_utc_ms)
    \/ (phase = "Dispatching") /\ \E admission_outcome \in DeliveryAdmissionOutcomeValues : \E at_utc_ms \in 0..2 : DispatchAcceptedFromDispatching(admission_outcome, at_utc_ms)
    \/ (phase = "Dispatching") /\ \E admission_outcome \in DeliveryAdmissionOutcomeValues : \E at_utc_ms \in 0..2 : DispatchDeduplicatedFromDispatching(admission_outcome, at_utc_ms)
    \/ (phase = "Dispatching") /\ \E at_utc_ms \in 0..2 : AwaitCompletionFromDispatching(at_utc_ms)
    \/ (phase = "Superseded") /\ \E at_utc_ms \in 0..2 : AwaitCompletionAfterSupersession(at_utc_ms)
    \/ (phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E at_utc_ms \in 0..2 : CompleteFromDispatchingOrAwaiting(at_utc_ms)
    \/ (phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionCompleted(outcome, detail, at_utc_ms)
    \/ (phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionRuntimeRejected(outcome, detail, at_utc_ms)
    \/ (phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionTransportError(outcome, detail, at_utc_ms)
    \/ (phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : RuntimeCompletionInternalError(outcome, detail, at_utc_ms)
    \/ (phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryCompletionFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryCompletionFailureTransportError(reason, detail, at_utc_ms)
    \/ (phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryCompletionFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryCompletionFailureInternalError(reason, detail, at_utc_ms)
    \/ (phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTargetMaterializationFailed(reason, detail, at_utc_ms)
    \/ (phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTargetMissing(reason, detail, at_utc_ms)
    \/ (phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTargetBusy(reason, detail, at_utc_ms)
    \/ (phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureRuntimeRejected(reason, detail, at_utc_ms)
    \/ (phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureMobRejected(reason, detail, at_utc_ms)
    \/ (phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureTransportError(reason, detail, at_utc_ms)
    \/ (phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DeliveryFailureInternalError(reason, detail, at_utc_ms)
    \/ (phase = "Claimed") /\ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeReadyClaimed(outcome, detail, at_utc_ms)
    \/ (phase = "Claimed") /\ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeBusyAllowedByPolicy(outcome, detail, at_utc_ms)
    \/ (phase = "Claimed") /\ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeBusySkipByPolicy(outcome, detail, at_utc_ms)
    \/ (phase = "Claimed") /\ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeMissingSkipByPolicy(outcome, detail, at_utc_ms)
    \/ (phase = "Claimed") /\ \E outcome \in OccurrenceTargetProbeOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : TargetProbeMissingMisfireByPolicy(outcome, detail, at_utc_ms)
    \/ (phase = "Pending") /\ \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : DueMisfirePending(detail, at_utc_ms)
    \/ (phase = "Pending" \/ phase = "Claimed" \/ phase = "Dispatching" \/ phase = "AwaitingCompletion") /\ \E arg_superseded_by_revision \in 0..2 : \E at_utc_ms \in 0..2 : SupersedePendingOrLive(arg_superseded_by_revision, at_utc_ms)
    \/ (phase = "Superseded") /\ \E arg_superseded_by_revision \in 0..2 : \E at_utc_ms \in 0..2 : SupersedeAlreadySuperseded(arg_superseded_by_revision, at_utc_ms)
    \/ (phase = "Superseded") /\ \E at_utc_ms \in 0..2 : LateCompleteAfterSupersession(at_utc_ms)
    \/ (phase = "Superseded") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionCompletedAfterSupersession(outcome, detail, at_utc_ms)
    \/ (phase = "Superseded") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionRejectedAfterSupersession(outcome, detail, at_utc_ms)
    \/ (phase = "Superseded") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionTransportErrorAfterSupersession(outcome, detail, at_utc_ms)
    \/ (phase = "Superseded") /\ \E outcome \in RuntimeCompletionOutcomeValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateRuntimeCompletionInternalErrorAfterSupersession(outcome, detail, at_utc_ms)
    \/ (phase = "Superseded") /\ \E reason \in DeliveryCompletionFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateDeliveryCompletionFailureAfterSupersession(reason, detail, at_utc_ms)
    \/ (phase = "Superseded") /\ \E reason \in DeliveryFailureReasonValues : \E detail \in OptionStringValues : \E at_utc_ms \in 0..2 : LateDeliveryFailureAfterSupersession(reason, detail, at_utc_ms)
    \/ (phase = "Dispatching") /\ \E arg_claim_token \in ClaimTokenValues : \E arg_lease_expires_at_utc_ms \in 0..2 : \E at_utc_ms \in 0..2 : RenewLeaseFromDispatching(arg_claim_token, arg_lease_expires_at_utc_ms, at_utc_ms)
    \/ (phase = "AwaitingCompletion") /\ \E arg_claim_token \in ClaimTokenValues : \E arg_lease_expires_at_utc_ms \in 0..2 : \E at_utc_ms \in 0..2 : RenewLeaseFromAwaitingCompletion(arg_claim_token, arg_lease_expires_at_utc_ms, at_utc_ms)
    \/ (phase = "Claimed") /\ \E at_utc_ms \in 0..2 : LeaseExpiredFromClaimed(at_utc_ms)
    \/ (phase = "Dispatching") /\ \E at_utc_ms \in 0..2 : LeaseExpiredFromDispatching(at_utc_ms)
    \/ (phase = "AwaitingCompletion") /\ \E at_utc_ms \in 0..2 : LeaseExpiredFromAwaitingCompletion(at_utc_ms)
    \/ (phase = "Claimed") /\ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromClaimed(at_utc_ms)
    \/ (phase = "Dispatching") /\ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromDispatching(at_utc_ms)
    \/ (phase = "AwaitingCompletion") /\ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleFromAwaitingCompletion(at_utc_ms)
    \/ (phase = "Superseded") /\ \E at_utc_ms \in 0..2 : ReleaseLeaseForPausedScheduleAfterSupersession(at_utc_ms)
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
