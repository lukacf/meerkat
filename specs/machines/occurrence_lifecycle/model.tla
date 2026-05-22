---- MODULE model ----
EXTENDS TLC, Naturals, Sequences, FiniteSets

\* Generated semantic machine model for OccurrenceLifecycleMachine.

CONSTANTS ClaimTokenValues, DeliveryReceiptStageValues, MisfirePolicyValues, MissingTargetPolicyValues, NatValues, OccurrenceFailureClassValues, OccurrenceIdValues, OccurrenceTransitionFailureObservationKindValues, OverlapPolicyValues, RuntimeCompletionOutcomeValues, ScheduleIdValues, SessionIdValues, StringValues

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

ClassifyTransitionFailurePlanRejectedPending(observation) ==
    /\ phase = "Pending"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedClaimed(observation) ==
    /\ phase = "Claimed"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedCompleted(observation) ==
    /\ phase = "Completed"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedSkipped(observation) ==
    /\ phase = "Skipped"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedMisfired(observation) ==
    /\ phase = "Misfired"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailurePlanRejectedDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ (observation = "PlanOccurrence")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedPending(observation) ==
    /\ phase = "Pending"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedClaimed(observation) ==
    /\ phase = "Claimed"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedCompleted(observation) ==
    /\ phase = "Completed"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedSkipped(observation) ==
    /\ phase = "Skipped"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedMisfired(observation) ==
    /\ phase = "Misfired"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ (observation = "SyncTargetSnapshot")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedPending(observation) ==
    /\ phase = "Pending"
    /\ (observation = "RecordReceipt")
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedClaimed(observation) ==
    /\ phase = "Claimed"
    /\ (observation = "RecordReceipt")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ (observation = "RecordReceipt")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ (observation = "RecordReceipt")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedCompleted(observation) ==
    /\ phase = "Completed"
    /\ (observation = "RecordReceipt")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedSkipped(observation) ==
    /\ phase = "Skipped"
    /\ (observation = "RecordReceipt")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedMisfired(observation) ==
    /\ phase = "Misfired"
    /\ (observation = "RecordReceipt")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ (observation = "RecordReceipt")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ (observation = "RecordReceipt")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedPending(observation) ==
    /\ phase = "Pending"
    /\ (observation = "ClassifyDue")
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedClaimed(observation) ==
    /\ phase = "Claimed"
    /\ (observation = "ClassifyDue")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ (observation = "ClassifyDue")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ (observation = "ClassifyDue")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedCompleted(observation) ==
    /\ phase = "Completed"
    /\ (observation = "ClassifyDue")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedSkipped(observation) ==
    /\ phase = "Skipped"
    /\ (observation = "ClassifyDue")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedMisfired(observation) ==
    /\ phase = "Misfired"
    /\ (observation = "ClassifyDue")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ (observation = "ClassifyDue")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ (observation = "ClassifyDue")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimPending(observation) ==
    /\ phase = "Pending"
    /\ (observation = "Claim")
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimClaimed(observation) ==
    /\ phase = "Claimed"
    /\ (observation = "Claim")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ (observation = "Claim")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ (observation = "Claim")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimCompleted(observation) ==
    /\ phase = "Completed"
    /\ (observation = "Claim")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimSkipped(observation) ==
    /\ phase = "Skipped"
    /\ (observation = "Claim")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimMisfired(observation) ==
    /\ phase = "Misfired"
    /\ (observation = "Claim")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ (observation = "Claim")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ (observation = "Claim")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedPending(observation) ==
    /\ phase = "Pending"
    /\ (observation = "DispatchStarted")
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedClaimed(observation) ==
    /\ phase = "Claimed"
    /\ (observation = "DispatchStarted")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ (observation = "DispatchStarted")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ (observation = "DispatchStarted")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedCompleted(observation) ==
    /\ phase = "Completed"
    /\ (observation = "DispatchStarted")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedSkipped(observation) ==
    /\ phase = "Skipped"
    /\ (observation = "DispatchStarted")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedMisfired(observation) ==
    /\ phase = "Misfired"
    /\ (observation = "DispatchStarted")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ (observation = "DispatchStarted")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotClaimedDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ (observation = "DispatchStarted")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingPending(observation) ==
    /\ phase = "Pending"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingClaimed(observation) ==
    /\ phase = "Claimed"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingCompleted(observation) ==
    /\ phase = "Completed"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingSkipped(observation) ==
    /\ phase = "Skipped"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingMisfired(observation) ==
    /\ phase = "Misfired"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotDispatchingDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ (observation = "AwaitCompletion")
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingPending(observation) ==
    /\ phase = "Pending"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingClaimed(observation) ==
    /\ phase = "Claimed"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingCompleted(observation) ==
    /\ phase = "Completed"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingSkipped(observation) ==
    /\ phase = "Skipped"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingMisfired(observation) ==
    /\ phase = "Misfired"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ ((observation = "LeaseExpired") \/ (observation = "ReleaseLeaseForPausedSchedule"))
    /\ phase' = "DeliveryFailed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalPending(observation) ==
    /\ phase = "Pending"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "Pending"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalClaimed(observation) ==
    /\ phase = "Claimed"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "Claimed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalDispatching(observation) ==
    /\ phase = "Dispatching"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "Dispatching"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(observation) ==
    /\ phase = "AwaitingCompletion"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "AwaitingCompletion"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalCompleted(observation) ==
    /\ phase = "Completed"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "Completed"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalSkipped(observation) ==
    /\ phase = "Skipped"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "Skipped"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalMisfired(observation) ==
    /\ phase = "Misfired"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "Misfired"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalSuperseded(observation) ==
    /\ phase = "Superseded"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
    /\ phase' = "Superseded"
    /\ model_step_count' = model_step_count + 1
    /\ UNCHANGED << occurrence_id, schedule_id, schedule_revision, occurrence_ordinal, trigger_key, target_binding_key, misfire_policy, misfire_policy_key, overlap_policy, overlap_policy_key, missing_target_policy, missing_target_policy_key, due_at_utc_ms, misfire_deadline_utc_ms, claimed_by, lease_expires_at_utc_ms, claimed_at_utc_ms, claim_token, delivery_correlation_id, target_materialized_session_id, receipt_recorded_at_utc_ms, last_receipt_recorded_at_utc_ms, last_receipt_attempt, last_receipt_stage, last_receipt_failure_class, last_receipt_detail, last_receipt_correlation_id, last_receipt_materialized_session_id, runtime_outcome_key, receipt_stage, receipt_failure_class, receipt_detail, failure_class, failure_detail, dispatched_at_utc_ms, completed_at_utc_ms, attempt_count, superseded_by_revision >>


ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(observation) ==
    /\ phase = "DeliveryFailed"
    /\ ((observation = "Complete") \/ (observation = "ResolveRuntimeCompletion") \/ (observation = "Skip") \/ (observation = "Misfire") \/ (observation = "Supersede") \/ (observation = "DeliveryFailed"))
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
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailurePlanRejectedDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureTargetSyncRejectedDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureReceiptRecordRejectedDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureDueClassificationRejectedDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotPendingForClaimDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotClaimedDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotDispatchingDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLeaseHoldingDeliveryFailed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalPending(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalClaimed(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalDispatching(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalAwaitingCompletion(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalCompleted(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalSkipped(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalMisfired(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalSuperseded(observation)
    \/ \E observation \in OccurrenceTransitionFailureObservationKindValues : ClassifyTransitionFailureNotLiveForTerminalDeliveryFailed(observation)
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
